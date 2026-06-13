//! # keymap-showcase — reducer (headless, testable state)
//!
//! This crate demonstrates all five keymap-rs values in one place.
//! `AppState` is the pure reducer: it owns all state and transitions
//! it given a `KeyInput`.  The `bin` (`main.rs`) is a thin ratatui
//! shell that owns the event loop and delegates every transition here.
//!
//! ## Five values wired here
//!
//! 1. **Declarative + dynamic merge** — defaults TOML + user TOML merged via
//!    `keymap_suite::merge`. Override notes are kept for display.
//! 2. **Dynamic rebind + tombstone** — `validate_rebind` guards the proposed
//!    key, then `Keymap::bind` or `.unbind`; saved via
//!    `to_toml_layered_with_unbinds` with atomic write (temp → rename).
//! 3. **Terminal survivability** — `legacy_lints` (keymap-core direct, API gap
//!    noted in SHOWCASE.md) runs over the global layer on init; canned bytes
//!    are decoded with `keymap_term::decode`.
//! 4. **Composite / timed sequences** — `TimedPending` drives a which-key panel;
//!    `continuations` lists follow-on keys. `deadline` feeds the event-loop
//!    timeout in `main.rs`.
//! 5. **Command palette** — `:` enters palette mode; `CommandIndex::complete`
//!    provides prefix candidates; Enter dispatches via `get`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use keymap_suite::{
    BuildError, CommandIndex, GLOBAL_LAYER, Key, KeyInput, Keymap, LegacyLint, Loaded, MergeNote,
    Merged, Modifiers, SequenceKeymap, TimedPending, Warning, legacy_lints,
};

// ── Default (built-in) config ─────────────────────────────────────────────────

const DEFAULTS_TOML: &str = r#"
[keys]
"ctrl+s" = "save"
"ctrl+q" = "quit"
"ctrl+z" = "undo"
"j"       = "move_down"
"k"       = "move_up"

[[sequences]]
keys   = ["ctrl+x", "ctrl+s"]
action = "save"

[[sequences]]
keys   = ["g", "g"]
action = "move_top"
"#;

// ── Actions ───────────────────────────────────────────────────────────────────

/// Every action the showcase recognises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Save the current document.
    Save,
    /// Quit the application.
    Quit,
    /// Undo the last change.
    Undo,
    /// Move the cursor down one line.
    MoveDown,
    /// Move the cursor up one line.
    MoveUp,
    /// Jump to the top of the document.
    MoveTop,
}

fn resolve_action(name: &str) -> Option<Action> {
    Some(match name {
        "save" => Action::Save,
        "quit" => Action::Quit,
        "undo" => Action::Undo,
        "move_down" => Action::MoveDown,
        "move_up" => Action::MoveUp,
        "move_top" => Action::MoveTop,
        _ => return None,
    })
}

/// Build a `CommandIndex` from the full action vocabulary (value 5: palette).
fn build_command_index() -> CommandIndex<Action> {
    let mut idx = CommandIndex::new();
    for (name, action) in [
        ("quit", Action::Quit),
        ("save", Action::Save),
        ("undo", Action::Undo),
        ("move_down", Action::MoveDown),
        ("move_up", Action::MoveUp),
        ("move_top", Action::MoveTop),
    ] {
        idx.bind(name, action);
    }
    idx
}

// ── Interaction mode ──────────────────────────────────────────────────────────

/// What the app is currently waiting for from the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Normal key dispatch through layers + sequences.
    Normal,
    /// Waiting to capture the next keypress as a new binding for `chord`.
    CaptureRebind {
        /// The chord being rebound (display use).
        chord: String,
    },
    /// Command-palette input: `:` prefix, text entered so far.
    Palette {
        /// Text the user has typed after `:`.
        text: String,
    },
}

// ── Legacy-lint summary (value 3) ─────────────────────────────────────────────

/// A single legacy-survivability finding for display.
#[derive(Clone, Debug)]
pub struct LegacyWarning {
    /// The chord in canonical form.
    pub chord: String,
    /// Whether the chord is unrepresentable or collapses to a twin.
    pub detail: String,
}

// ── AppState ──────────────────────────────────────────────────────────────────

/// All mutable application state. Transitions are pure methods: they take
/// `&mut self` and a `KeyInput`, and return an `Outcome`.  No clock, no I/O
/// inside the reducer — both stay with the caller (`main.rs`).
pub struct AppState {
    // Value 1 & 2: merged keymap (defaults ⊕ user).
    /// The live global-layer keymap (defaults merged with user overrides).
    pub global: Keymap<Action>,
    /// The sequence map (defaults merged with user sequences).
    pub sequences: SequenceKeymap<Action>,
    /// Unbind declarations accumulated by the user (tombstones for save).
    pub unbinds: Vec<KeyInput>,
    /// Override notes from the last `merge` call.
    pub merge_notes: Vec<MergeNote>,
    /// Warnings from the last load.
    pub warnings: Vec<Warning>,

    // Value 3: terminal survivability.
    /// Legacy-lint findings on the current global layer.
    pub legacy_warnings: Vec<LegacyWarning>,
    /// Decoded demo: bytes `\x1b[A` decoded to a `KeyInput`.
    pub decode_demo: String,

    // Value 4: timed sequences.
    /// The sequence pending-buffer (owned here; fed per key in the reducer).
    pub pending: TimedPending,

    // Value 5: command palette.
    /// Command index built from the full action vocabulary.
    pub commands: CommandIndex<Action>,

    // Interaction.
    /// Current interaction mode.
    pub mode: Mode,
    /// Status/feedback message shown in the UI.
    pub status: String,
    /// Whether the app should exit.
    pub should_quit: bool,

    // Persistence.
    /// Where to write the user config (test: temp dir; prod: ~/.config/…).
    save_path: PathBuf,
}

/// The outcome of processing one key in the reducer.
#[derive(Debug)]
pub enum Outcome {
    /// The key fired an action.
    Fired(Action),
    /// The key advanced a pending sequence; nothing resolved yet.
    SequencePending,
    /// A pending sequence expired (timeout); the buffered keys fell through.
    SequenceExpired,
    /// The key fell through all layers and sequences.
    Passthrough,
    /// The app should quit.
    Quit,
}

impl AppState {
    /// Creates a new `AppState` by loading defaults and optional user config,
    /// then merging them.
    ///
    /// `save_path` is where the user config is written when the user saves a
    /// rebind. In tests, pass a temp-dir path; in production, pass the user
    /// config directory.
    ///
    /// # Errors
    ///
    /// Returns `BuildError` only if the built-in defaults TOML fails to parse,
    /// which cannot happen at runtime (it is a compile-time constant).
    ///
    /// # Panics
    ///
    /// Panics if `from_toml_str` fails to parse the empty string `""`, which
    /// would indicate a bug in the TOML parser (empty input always produces an
    /// empty config).
    pub fn new(save_path: PathBuf) -> Result<Self, BuildError> {
        // Value 1: load defaults and merge with user config (if present).
        let defaults: Loaded<Action> = keymap_suite::from_toml_str(DEFAULTS_TOML, resolve_action)?;

        let merged: Merged<Action> = if save_path.exists() {
            match std::fs::read_to_string(&save_path)
                .ok()
                .and_then(|s| keymap_suite::from_toml_str(&s, resolve_action).ok())
            {
                Some(user) => keymap_suite::merge(defaults, user),
                // User file unreadable or unparseable — merge an empty overlay
                // so the output is just the defaults with no notes.
                None => keymap_suite::merge(
                    defaults,
                    keymap_suite::from_toml_str("", resolve_action)
                        .expect("empty TOML always parses"),
                ),
            }
        } else {
            // No user file yet — merge with empty overlay (identity merge).
            keymap_suite::merge(
                defaults,
                keymap_suite::from_toml_str("", resolve_action).expect("empty TOML always parses"),
            )
        };

        let global = merged
            .output
            .layers
            .get(GLOBAL_LAYER)
            .cloned()
            .unwrap_or_default();
        let sequences = merged.output.sequences;
        let warnings = merged.output.warnings;
        let merge_notes = merged.notes;
        let unbinds: Vec<KeyInput> = merged
            .output
            .unbinds
            .get(GLOBAL_LAYER)
            .cloned()
            .unwrap_or_default();

        // Value 3: legacy-lints on the global layer (keymap-core direct — API gap).
        let legacy_warnings = compute_legacy_warnings(&global);

        // Value 3: canned-bytes decode demo (~10 lines, headless).
        let decode_demo = decode_canned_bytes();

        // Value 5: command palette index.
        let commands = build_command_index();

        Ok(AppState {
            global,
            sequences,
            unbinds,
            merge_notes,
            warnings,
            legacy_warnings,
            decode_demo,
            pending: TimedPending::new(),
            commands,
            mode: Mode::Normal,
            status: String::new(),
            should_quit: false,
            save_path,
        })
    }

    // ── Value 4: sequence / which-key ─────────────────────────────────────

    /// The continuations reachable from the current pending prefix (value 4).
    /// Empty when no sequence is in progress.
    #[must_use]
    pub fn continuations(&self) -> Vec<String> {
        use keymap_suite::Match;
        let buf = self.pending.pending();
        if buf.is_empty() {
            return vec![];
        }
        match self.sequences.lookup(buf) {
            Match::Prefix => self
                .sequences
                .continuations(buf)
                .map(|(key, _)| key.to_string())
                .collect(),
            _ => vec![],
        }
    }

    // ── Value 5: palette candidates ────────────────────────────────────────

    /// Returns the prefix-completion candidates for the current palette text.
    #[must_use]
    pub fn palette_candidates(&self) -> Vec<(&str, &Action)> {
        if let Mode::Palette { text } = &self.mode {
            self.commands.complete(text).collect()
        } else {
            vec![]
        }
    }

    // ── Key dispatch ───────────────────────────────────────────────────────

    /// Process one `KeyInput` (already decoded from the terminal), advancing
    /// state and returning an `Outcome`. The `now` / `window` arguments are
    /// used for sequence timeout (value 4); the caller supplies them so the
    /// reducer stays clock-free.
    pub fn feed(
        &mut self,
        key: KeyInput,
        now: std::time::Instant,
        window: std::time::Duration,
    ) -> Outcome {
        match &self.mode.clone() {
            Mode::CaptureRebind { chord } => {
                self.apply_rebind(chord, key);
                Outcome::Passthrough
            }
            Mode::Palette { text: _ } => self.feed_palette(key),
            Mode::Normal => self.feed_normal(key, now, window),
        }
    }

    fn feed_normal(
        &mut self,
        key: KeyInput,
        now: std::time::Instant,
        window: std::time::Duration,
    ) -> Outcome {
        use keymap_suite::Step;

        // Enter palette on `:`.
        if key == KeyInput::new(Key::Char(':'), Modifiers::NONE) {
            self.mode = Mode::Palette {
                text: String::new(),
            };
            self.status = "palette: type a command name, Enter to run, Esc to cancel".to_string();
            return Outcome::Passthrough;
        }

        // Value 4: feed into timed sequence buffer.
        let timed = self.pending.feed(&self.sequences, key, now, window);

        // Handle expired prefix (timeout fired).
        let expired_note = if timed.expired.is_some() {
            Some(Outcome::SequenceExpired)
        } else {
            None
        };

        match timed.step {
            Step::Fired(action) => {
                if *action == Action::Quit {
                    self.should_quit = true;
                    return Outcome::Quit;
                }
                self.status = format!("fired: {action:?}");
                Outcome::Fired(action.clone())
            }
            Step::PassThrough(keys) => {
                // Try single-chord dispatch through the global layer.
                if let Some(k) = keys.last() {
                    if let Some(action) = self.global.get(k) {
                        if *action == Action::Quit {
                            self.should_quit = true;
                            return Outcome::Quit;
                        }
                        self.status = format!("fired: {action:?}");
                        return Outcome::Fired(action.clone());
                    }
                }
                expired_note.unwrap_or(Outcome::Passthrough)
            }
            Step::Pending => {
                self.status = format!(
                    "sequence pending: {} key(s) — continuations: {}",
                    self.pending.pending().len(),
                    self.continuations().join(", ")
                );
                Outcome::SequencePending
            }
        }
    }

    fn feed_palette(&mut self, key: KeyInput) -> Outcome {
        // Esc cancels.
        if key == KeyInput::new(Key::Esc, Modifiers::NONE) {
            self.mode = Mode::Normal;
            self.status = "palette cancelled".to_string();
            return Outcome::Passthrough;
        }
        // Enter dispatches.
        if key == KeyInput::new(Key::Enter, Modifiers::NONE) {
            let text = if let Mode::Palette { text } = &self.mode {
                text.clone()
            } else {
                String::new()
            };
            self.mode = Mode::Normal;
            if let Some(action) = self.commands.get(&text) {
                if *action == Action::Quit {
                    self.should_quit = true;
                    return Outcome::Quit;
                }
                self.status = format!("palette → {action:?}");
                return Outcome::Fired(action.clone());
            }
            self.status = format!("palette: unknown command '{text}'");
            return Outcome::Passthrough;
        }
        // Backspace.
        if key == KeyInput::new(Key::Backspace, Modifiers::NONE) {
            if let Mode::Palette { text } = &mut self.mode {
                text.pop();
            }
            return Outcome::Passthrough;
        }
        // Printable char → append.
        if let Key::Char(c) = key.key() {
            if key.modifiers() == Modifiers::NONE {
                if let Mode::Palette { text } = &mut self.mode {
                    text.push(c);
                }
            }
        }
        Outcome::Passthrough
    }

    // ── Value 2: rebind ────────────────────────────────────────────────────

    /// Begin capturing the next keypress as a rebind target for `chord`.
    ///
    /// Call this when the user selects a chord to rebind; the next `feed` call
    /// will invoke `apply_rebind`.
    pub fn start_rebind(&mut self, chord: String) {
        self.mode = Mode::CaptureRebind { chord };
        self.status = "press any key to rebind (Esc = cancel)".to_string();
    }

    /// Begin a tombstone (delete) for `chord`.  Removes the chord from the live
    /// map and records it in `unbinds` for the next save.
    pub fn delete_binding(&mut self, chord: &KeyInput) {
        self.global.unbind(chord);
        if !self.unbinds.contains(chord) {
            self.unbinds.push(*chord);
        }
        self.legacy_warnings = compute_legacy_warnings(&self.global);
        self.status = format!("deleted binding for {chord}");
    }

    fn apply_rebind(&mut self, target_chord_str: &str, new_key: KeyInput) {
        use keymap_suite::RebindVerdict;
        self.mode = Mode::Normal;

        // Cancel on Esc.
        if new_key == KeyInput::new(Key::Esc, Modifiers::NONE) {
            self.status = "rebind cancelled".to_string();
            return;
        }

        // Value 2: validate_rebind guards the reserved set.
        let reserved = reserved_keys();
        let layers = [&self.global];
        let verdict = keymap_suite::validate_rebind(&layers, 0, new_key, &reserved);

        match verdict {
            RebindVerdict::BreaksReserved { reserved: r, .. } => {
                self.status = format!("rebind refused: {r} is reserved");
            }
            RebindVerdict::Allowed { shadows, .. } => {
                // Materialise the shadow name before releasing the borrow on
                // `self.global` that `layers` holds (needed so the mutable
                // `unbind`/`bind` calls below can proceed).
                let shadow_name: Option<String> = shadows.map(|a| action_name(a).to_string());

                // Look up the action being rebound.
                if let Ok(target) = target_chord_str.parse::<KeyInput>() {
                    if let Some(action) = self.global.unbind(&target) {
                        self.global.bind(new_key, action);
                        self.legacy_warnings = compute_legacy_warnings(&self.global);
                        // Surface the shadow advisory: if the proposed key already
                        // had a different action bound to it, tell the user what was
                        // overwritten (value②: safe dynamic change is more complete
                        // with the "X shadows Y" UX surface).
                        if let Some(shadowed) = shadow_name {
                            self.status = format!(
                                "rebound {target_chord_str} → {new_key} (overwrote {shadowed})"
                            );
                        } else {
                            self.status = format!("rebound {target_chord_str} → {new_key}");
                        }
                    } else {
                        self.status = format!("rebind: {target_chord_str} had no binding");
                    }
                }
            }
            // Non-exhaustive: future verdict variants are silently ignored.
            _ => {}
        }
    }

    // ── Value 1 + 2: save ─────────────────────────────────────────────────

    /// Serialises the current keymap + tombstones to the user config path using
    /// an atomic write (temp file → rename), so a crash mid-write never
    /// corrupts the config file.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if the write or rename fails.
    pub fn save_config(&self) -> std::io::Result<()> {
        let mut layers = BTreeMap::new();
        layers.insert(GLOBAL_LAYER.to_string(), self.global.clone());

        let mut unbinds_map: BTreeMap<String, Vec<KeyInput>> = BTreeMap::new();
        if !self.unbinds.is_empty() {
            unbinds_map.insert(GLOBAL_LAYER.to_string(), self.unbinds.clone());
        }

        // Value 2: to_toml_layered_with_unbinds emits tombstones.
        let toml_str = keymap_suite::to_toml_layered_with_unbinds(
            &layers,
            &self.sequences,
            &unbinds_map,
            |a| Some(action_name(a)),
        );

        // Atomic write: temp file → rename (trusted-path, no PTY-writable dir).
        let tmp = self.save_path.with_extension("tmp");
        if let Some(parent) = tmp.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp, toml_str)?;
        std::fs::rename(&tmp, &self.save_path)?;
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The reserved key set: these chords can never be rebound.
#[must_use]
pub fn reserved_keys() -> Vec<KeyInput> {
    vec![
        KeyInput::new(Key::Char('c'), Modifiers::CTRL),
        KeyInput::new(Key::Esc, Modifiers::NONE),
    ]
}

/// Returns the canonical action name for serialisation.
fn action_name(action: &Action) -> &str {
    match action {
        Action::Save => "save",
        Action::Quit => "quit",
        Action::Undo => "undo",
        Action::MoveDown => "move_down",
        Action::MoveUp => "move_up",
        Action::MoveTop => "move_top",
    }
}

/// Runs `legacy_lints` over the global layer (value 3).
///
/// Uses `keymap_suite::legacy_lints` — the gap that previously required a
/// direct `keymap-core` dependency has been closed (see SHOWCASE.md).
fn compute_legacy_warnings(global: &Keymap<Action>) -> Vec<LegacyWarning> {
    legacy_lints(global)
        .into_iter()
        .map(|lint| match lint {
            LegacyLint::Unrepresentable { chord } => LegacyWarning {
                chord,
                detail: "unrepresentable in legacy terminals".to_string(),
            },
            LegacyLint::CollapsesTo {
                chord,
                collapses_to,
            } => LegacyWarning {
                chord,
                detail: format!("collapses to {collapses_to} in legacy terminals"),
            },
            _ => LegacyWarning {
                chord: String::new(),
                detail: String::new(),
            },
        })
        .filter(|lw| !lw.chord.is_empty())
        .collect()
}

/// Decodes a canned escape sequence (`ESC [ A` = cursor-up) and returns a
/// description string for display (value 3, ~10 lines, headless).
fn decode_canned_bytes() -> String {
    use keymap_term::DecodeMode;
    use keymap_term::decode;

    let bytes = b"\x1b[A"; // CSI A = cursor-up (arrow-up) in baseline terminal.
    let result = decode(bytes, DecodeMode::Baseline);
    format!("decode(ESC[A) → {result:?}")
}

// ── Returns the platform user-config path ─────────────────────────────────────

/// Returns the default user config path for the showcase
/// (`~/.config/keymap-showcase/user.toml`), or `None` if `$HOME` is not set.
///
/// When `$HOME` is absent the function returns `None` rather than falling back
/// to `.` (the current working directory), because the CWD may be a
/// PTY-writable location in a multiplexer or remote session, which could
/// inadvertently write config files into untrusted directories.
/// The caller should treat `None` as "saving is disabled for this session".
#[must_use]
pub fn default_save_path() -> Option<PathBuf> {
    save_path_from_home(std::env::var("HOME").ok().as_deref())
}

/// Inner pure helper: builds the config path from an optional home directory
/// string. Returns `None` when `home` is `None`.
///
/// Extracted so tests can call it without manipulating environment variables.
fn save_path_from_home(home: Option<&str>) -> Option<PathBuf> {
    let home = home?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("keymap-showcase")
            .join("user.toml"),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use keymap_suite::WarningKind;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_path() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "keymap-showcase-test-{}-{}.toml",
            std::process::id(),
            n,
        ))
    }

    fn app() -> AppState {
        AppState::new(tmp_path()).expect("defaults must parse")
    }

    fn now() -> Instant {
        Instant::now()
    }

    fn window() -> Duration {
        Duration::from_millis(500)
    }

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    fn plain(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::NONE)
    }

    // ── Value 1: merge ───────────────────────────────────────────────────────

    /// Defaults TOML loads cleanly; the global layer has the built-in bindings.
    #[test]
    fn value1_defaults_load_and_global_has_builtin_bindings() {
        let app = app();
        assert_eq!(app.global.get(&ctrl('s')), Some(&Action::Save));
        assert_eq!(app.global.get(&ctrl('q')), Some(&Action::Quit));
    }

    /// A user TOML that overrides `ctrl+s` wins after merge, and the override
    /// appears in `merge_notes`.
    #[test]
    fn value1_user_toml_override_wins_and_note_recorded() {
        let path = tmp_path();
        // Write a user config that overrides ctrl+s → undo.
        std::fs::write(&path, "[keys]\n\"ctrl+s\" = \"undo\"\n").unwrap();
        let app = AppState::new(path.clone()).expect("valid user toml");
        std::fs::remove_file(&path).ok();

        assert_eq!(
            app.global.get(&ctrl('s')),
            Some(&Action::Undo),
            "user override must win"
        );
        assert!(
            app.merge_notes
                .iter()
                .any(|n| matches!(n, MergeNote::Overrode { chord, .. } if chord == "ctrl+s")),
            "Overrode note expected: {:?}",
            app.merge_notes
        );
    }

    /// Warnings from load are accessible; `WarningKind` categorises them.
    #[test]
    fn value1_warnings_accessible_with_warning_kind() {
        let path = tmp_path();
        std::fs::write(&path, "[keys]\n\"ctrl+s\" = \"no_such_action\"\n").unwrap();
        let app = AppState::new(path.clone()).expect("valid toml");
        std::fs::remove_file(&path).ok();

        // The merge produces an UnknownAction warning for "no_such_action".
        assert!(
            app.warnings
                .iter()
                .any(|w| w.kind() == WarningKind::UnknownAction),
            "expected UnknownAction warning: {:?}",
            app.warnings
        );
    }

    // ── Value 2: rebind + tombstone + save ───────────────────────────────────

    /// `validate_rebind` accepts a non-reserved key.
    #[test]
    fn value2_validate_rebind_allows_non_reserved() {
        use keymap_suite::{RebindVerdict, validate_rebind};

        let app = app();
        let layers = [&app.global];
        let proposed = KeyInput::new(Key::Char('w'), Modifiers::CTRL);
        let reserved = reserved_keys();
        let verdict = validate_rebind(&layers, 0, proposed, &reserved);
        assert!(
            matches!(verdict, RebindVerdict::Allowed { .. }),
            "ctrl+w is not reserved: {verdict:?}"
        );
    }

    /// `validate_rebind` refuses a reserved key.
    #[test]
    fn value2_validate_rebind_refuses_reserved() {
        use keymap_suite::{BreakReason, RebindVerdict, validate_rebind};

        let app = app();
        let layers = [&app.global];
        let esc = KeyInput::new(Key::Esc, Modifiers::NONE);
        let reserved = reserved_keys();
        let verdict = validate_rebind(&layers, 0, esc, &reserved);
        assert!(
            matches!(
                verdict,
                RebindVerdict::BreaksReserved {
                    reason: BreakReason::DirectSteal,
                    ..
                }
            ),
            "Esc is reserved: {verdict:?}"
        );
    }

    /// `delete_binding` removes the chord from the live map and adds it to unbinds.
    #[test]
    fn value2_delete_binding_removes_from_map_and_adds_to_unbinds() {
        let mut app = app();
        let s = ctrl('s');
        assert!(
            app.global.get(&s).is_some(),
            "ctrl+s must exist before delete"
        );
        app.delete_binding(&s);
        assert!(
            app.global.get(&s).is_none(),
            "ctrl+s must be gone after delete"
        );
        assert!(app.unbinds.contains(&s), "unbinds must record the chord");
    }

    /// `save_config` writes a file that can be loaded back; tombstones survive
    /// the round-trip.
    #[test]
    fn value2_save_config_round_trips_tombstone() {
        let path = tmp_path();
        let mut app = AppState::new(path.clone()).expect("defaults parse");

        // Delete ctrl+z (value 2 tombstone).
        let z = ctrl('z');
        app.delete_binding(&z);
        app.save_config().expect("save must succeed");

        // Reload and check the tombstone is in unbinds.
        let reloaded = AppState::new(path.clone()).expect("reload parse");
        std::fs::remove_file(&path).ok();
        // After merge, unbinds from the user config are applied to defaults:
        // ctrl+z should no longer be in the global layer.
        assert!(
            reloaded.global.get(&z).is_none(),
            "tombstone must survive save/reload: ctrl+z should be absent"
        );
    }

    /// When `apply_rebind` assigns a key that already has a different action,
    /// the status message surfaces the shadow advisory ("overwrote <action>").
    #[test]
    fn value2_apply_rebind_surfaces_shadow_in_status() {
        let mut app = app();
        // The default map has ctrl+s → Save. Rebind ctrl+s to something else by
        // starting a capture for "ctrl+q" and pressing ctrl+s as the new key.
        // ctrl+s shadows the existing Save binding.
        app.start_rebind("ctrl+q".to_string());
        let new_key = ctrl('s'); // ctrl+s is already bound to Save (not reserved)
        app.apply_rebind("ctrl+q", new_key);

        // The status must mention the shadow.
        assert!(
            app.status.contains("overwrote"),
            "status must surface shadow advisory: {:?}",
            app.status
        );
        // ctrl+q should now be gone (unbound) and ctrl+s should carry quit.
        assert!(
            app.global.get(&ctrl('q')).is_none(),
            "ctrl+q must be unbound after rebind"
        );
        assert_eq!(
            app.global.get(&ctrl('s')),
            Some(&Action::Quit),
            "ctrl+s now carries quit"
        );
    }

    // ── Value 3: legacy lints + decode demo ──────────────────────────────────

    /// `legacy_warnings` is computed on init. The default bindings contain no
    /// collapsing chords (ctrl+s, ctrl+q, ctrl+z, j, k are all representable),
    /// so the default set is empty. Additionally we verify that a known
    /// collapsing chord (`ctrl+i` → `tab`) is detected when present.
    #[test]
    fn value3_legacy_lints_run_on_init() {
        let app = app();
        // Default bindings produce no legacy lints.
        assert!(
            app.legacy_warnings.is_empty(),
            "default bindings should produce no legacy lints: {:?}",
            app.legacy_warnings
        );

        // Adding ctrl+i (which collapses to Tab in legacy C0) must produce a lint.
        let mut km = Keymap::new();
        km.bind(KeyInput::new(Key::Char('i'), Modifiers::CTRL), Action::Save);
        let lints = legacy_lints(&km);
        assert!(
            !lints.is_empty(),
            "ctrl+i should produce at least one legacy lint"
        );
        // The lint must describe the collapse to Tab.
        let has_collapse = lints.iter().any(|lint| {
            matches!(
                lint,
                LegacyLint::CollapsesTo { collapses_to, .. }
                    if collapses_to == "tab"
            )
        });
        assert!(
            has_collapse,
            "ctrl+i must collapse to tab in legacy mode: {lints:?}"
        );
    }

    /// The decode demo decodes `ESC[A` to cursor-up (`Key::Up`).
    #[test]
    fn value3_decode_demo_is_non_empty() {
        use keymap_term::{DecodeMode, Decoded, decode};
        // Verify the canned bytes decode to Up with no modifiers.
        let bytes = b"\x1b[A";
        let result = decode(bytes, DecodeMode::Baseline);
        match result {
            Decoded::Key { input, consumed } => {
                assert_eq!(
                    input,
                    KeyInput::new(Key::Up, Modifiers::NONE),
                    "ESC[A must decode to Key::Up: {input:?}"
                );
                assert_eq!(consumed, 3, "ESC[A is 3 bytes");
            }
            other => panic!("ESC[A should decode to a Key, got {other:?}"),
        }
        // Also verify the AppState decode_demo string reflects this.
        let app = app();
        assert!(
            !app.decode_demo.is_empty(),
            "decode demo must produce output"
        );
        assert!(
            app.decode_demo.contains("Up"),
            "decode demo should mention Up: {}",
            app.decode_demo
        );
    }

    // ── Value 4: timed sequences ─────────────────────────────────────────────

    /// Pressing `g` advances into a pending prefix; continuations lists `g`.
    #[test]
    fn value4_first_key_of_sequence_enters_pending() {
        let mut app = app();
        let t = now();
        let outcome = app.feed(plain('g'), t, window());
        assert!(
            matches!(outcome, Outcome::SequencePending),
            "g starts a sequence: {outcome:?}"
        );
        assert!(
            !app.continuations().is_empty(),
            "continuations must be non-empty after first g"
        );
    }

    /// Pressing `g g` fires the sequence action `move_top`.
    #[test]
    fn value4_gg_sequence_fires_move_top() {
        let mut app = app();
        let t = now();
        let _first = app.feed(plain('g'), t, window());
        let outcome = app.feed(plain('g'), t, window());
        assert!(
            matches!(outcome, Outcome::Fired(Action::MoveTop)),
            "g g must fire MoveTop: {outcome:?}"
        );
    }

    // ── Value 5: command palette ─────────────────────────────────────────────

    /// Pressing `:` enters palette mode.
    #[test]
    fn value5_colon_enters_palette_mode() {
        let mut app = app();
        let t = now();
        let _ = app.feed(plain(':'), t, window());
        assert!(
            matches!(app.mode, Mode::Palette { .. }),
            "colon must enter palette: {:?}",
            app.mode
        );
    }

    /// `palette_candidates` returns completions for a prefix.
    #[test]
    fn value5_palette_completions_for_prefix() {
        let mut app = app();
        app.mode = Mode::Palette {
            text: "mo".to_string(),
        };
        let candidates: Vec<&str> = app
            .palette_candidates()
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert!(
            candidates.iter().any(|n| n.starts_with("mo")),
            "completions for 'mo' expected: {candidates:?}"
        );
    }

    /// `Enter` in palette mode dispatches the exact-match command.
    #[test]
    fn value5_palette_enter_dispatches_exact_command() {
        let mut app = app();
        let t = now();
        // Enter palette.
        app.feed(plain(':'), t, window());
        // Type "undo".
        for c in "undo".chars() {
            app.feed(plain(c), t, window());
        }
        // Enter to dispatch.
        let outcome = app.feed(KeyInput::new(Key::Enter, Modifiers::NONE), t, window());
        assert!(
            matches!(outcome, Outcome::Fired(Action::Undo)),
            "palette must dispatch Undo: {outcome:?}"
        );
    }

    // ── default_save_path ────────────────────────────────────────────────────

    /// When $HOME is set, `default_save_path` returns a path under ~/.config.
    #[test]
    fn default_save_path_with_home_returns_path_under_config() {
        let path = save_path_from_home(Some("/home/user"));
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(
            path.starts_with("/home/user/.config"),
            "path must be under ~/.config: {path:?}"
        );
        assert!(
            path.ends_with("user.toml"),
            "path must end in user.toml: {path:?}"
        );
    }

    /// When $HOME is absent, `default_save_path` returns `None` — no fallback
    /// to the current working directory, which may be PTY-writable.
    #[test]
    fn default_save_path_without_home_returns_none() {
        let path = save_path_from_home(None);
        assert!(
            path.is_none(),
            "absent $HOME must return None, not a CWD fallback"
        );
    }
}
