//! # keymap-suite — the one-import facade for `keymap-rs`
//!
//! `keymap-suite` is the convenience layer that bundles `keymap-core`,
//! `keymap-config`, and `keymap-seq` into a single import for the
//! nine-out-of-ten case: **load TOML, resolve a key against the active layers,
//! handle the multi-key sequence buffer, and forward warnings to the caller**.
//! Authors who need to drop a level lower (write their own backend, decode raw
//! terminal bytes, do empirical reachability work) depend on the underlying
//! crates directly.
//!
//! ```no_run
//! use keymap_suite::prelude::*;
//!
//! #[derive(Clone, Debug, PartialEq)]
//! enum Action { Quit, Save }
//!
//! let loaded = keymap_suite::from_toml_path("keys.toml", |name| match name {
//!     "quit" => Some(Action::Quit),
//!     "save" => Some(Action::Save),
//!     _ => None,
//! })?;
//!
//! // The active layer chain is *yours* — pick what the current context wants.
//! let chain = [loaded.global()];
//! # let key = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
//! if let Some(action) = resolve_layered(chain.iter().copied(), &key) {
//!     println!("fire {action:?}");
//! }
//! # Ok::<(), keymap_suite::LoadError>(())
//! ```
//!
//! The same example with inline TOML — compiles and runs as-is (tested in CI):
//!
//! ```
//! use keymap_suite::prelude::*;
//!
//! #[derive(Clone, Debug, PartialEq)]
//! enum Action { Save, CommandPalette, OpenFile }
//!
//! fn resolve(name: &str) -> Option<Action> {
//!     match name {
//!         "save"            => Some(Action::Save),
//!         "command_palette" => Some(Action::CommandPalette),
//!         "open_file"       => Some(Action::OpenFile),
//!         _ => None,
//!     }
//! }
//!
//! const SETTINGS: &str = r#"
//! [keys]
//! "ctrl+s" = "save"
//! "ctrl+p" = "command_palette"
//!
//! [layers.panel]
//! "ctrl+p" = "open_file"
//!
//! [[sequences]]
//! keys   = ["ctrl+x", "ctrl+s"]
//! action = "save"
//! "#;
//!
//! let loaded = keymap_suite::from_toml_str(SETTINGS, resolve)?;
//!
//! // Assemble the active chain from your application state (not the library's).
//! let panel_focused = true;
//! let chain: Vec<&Keymap<Action>> = if panel_focused {
//!     vec![&loaded.layers["panel"], loaded.global()]
//! } else {
//!     vec![loaded.global()]
//! };
//! let key = chords::ctrl('p');
//! if let Some(action) = resolve_layered(chain.iter().copied(), &key) {
//!     assert_eq!(action, &Action::OpenFile); // panel wins
//! }
//!
//! // Multi-key sequence: feed keys one at a time.
//! let mut pending = loaded.pending_sequence();
//! let cx = chords::ctrl('x');
//! let cs = chords::ctrl('s');
//! let step1 = pending.feed(&loaded.sequences, cx);
//! assert!(matches!(step1, Step::Pending));
//! let step2 = pending.feed(&loaded.sequences, cs);
//! assert!(matches!(step2, Step::Fired(_)));
//!
//! # Ok::<(), keymap_suite::BuildError>(())
//! ```
//!
//! ## What is in the suite, and what is not
//!
//! - **In**: the TOML loader, layered resolution, the multi-key sequence buffer
//!   (`PendingSequence` / `Step`, and the timeout-aware `TimedPending` /
//!   `TimedStep`), the canonical key vocabulary (`Key`, `KeyInput`, `Modifiers`),
//!   terse chord constructors ([`chords`]), reverse lookup for help screens
//!   ([`keys_for_action`]), the warnings the loader collects (with category tags
//!   via [`WarningKind`]), and a `prelude` that pulls them in with one `use`. The
//!   optional `crossterm` feature adds the
//!   `KeyInput::try_from(crossterm::event::KeyEvent)` adapter.
//! - **Also reachable from root** (not in prelude — more specialised):
//!   [`CommandIndex`] for command-palette prefix lookup;
//!   [`validate_rebind`] / [`RebindVerdict`] / [`BreakReason`] / [`LegacyForm`]
//!   for opt-in rebind safety checks; [`merge`] / [`Merged`] / [`MergeNote`] for
//!   layering a user config over defaults; [`to_toml_layered_with_unbinds`] for
//!   round-tripping configs that include tombstone (`= false`) entries;
//!   [`legacy_lints`] / [`LegacyLint`] for the static terminal-survivability
//!   diagnostic pass (opt-in, not per-event).
//! - **Out (on purpose)**: PTY byte decoding lives in `keymap-term`, not here;
//!   runtime state (the active layer chain, the inter-key timer driving
//!   `PendingSequence::flush`) stays with the caller, exactly as the rest of
//!   `keymap-rs` is designed.
//!
//! ## crossterm
//!
//! Most TUI authors read events through `crossterm`. Enable the `crossterm`
//! feature and `KeyInput::try_from(key_event)` is available — it just turns on
//! `keymap-core`'s own `crossterm` feature (where the adapter lives) and
//! re-exports [`UnsupportedKey`], the error a key with no neutral form converts
//! to. The default build stays backend-neutral and pulls in no crossterm.
//!
//! ```toml
//! keymap-suite = { version = "0.1", features = ["crossterm"] }
//! ```
//!
//! ## Warnings: lenient by default, opt into strict
//!
//! [`from_toml_str`] and [`from_toml_path`] return a [`Loaded`] whose
//! [`warnings`](Loaded::warnings) field carries every non-fatal problem the
//! loader saw (chord conflicts inside one layer, unknown action names,
//! sequence prefix shadows, …). The caller decides what to do with them;
//! exactly the same contract `keymap-config` has, so the same TOML file
//! behaves identically whether you load it via this suite or the underlying
//! crate.
//!
//! If you want a strict gate (CI hook, production startup) one call expresses
//! it:
//!
//! ```
//! use keymap_suite::prelude::*;
//!
//! # #[derive(Clone, Debug, PartialEq)] enum Action { Save }
//! # let toml = r#""#;
//! let loaded = keymap_suite::from_toml_str(toml, |_| None::<Action>)?
//!     .deny_warnings()
//!     .map_err(|warnings| format!("{} keymap warning(s); aborting", warnings.len()))?;
//! # let _ = loaded;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## State stays with you
//!
//! The active layer chain (`[&panel, &global]`) is yours to assemble per event
//! because the *meaning* of a layer is yours: it knows your popup-vs-normal,
//! your mode, your focus. The suite never invents a layer-stack type.
//!
//! The multi-key pending buffer is held by [`PendingSequence`], also yours to
//! own as a field. Its [`feed`](PendingSequence::feed) takes the
//! [`SequenceKeymap`] per call rather than borrowing it, so the
//! `struct App { map, pending }` colocation pattern works without
//! self-referential gymnastics. The library holds no clock — when your inter-
//! key timer fires, you call [`flush`](PendingSequence::flush) yourself.

#![doc(html_root_url = "https://docs.rs/keymap-suite/0.1.0")]

use std::path::Path;

// --- Re-exports: the curated public vocabulary. ---
//
// We re-export by *selection*, not by glob, so the suite's surface grows only
// when this file does. A downstream addition to `keymap-core` (etc.) cannot
// silently widen the suite — which keeps the suite's semver promise its own.
//
// Within each `pub use` line items are alphabetised so rustfmt does not
// reorder them; the *line breaks* group related items (vocabulary → tables →
// errors → sequences) for readers, not for the compiler.

pub use keymap_core::{Key, KeyInput, Modifiers, ParseKeyInputError};
pub use keymap_core::{Keymap, resolve_layered};
// Opt-in rebind validation. `LegacyForm` is included because `RebindVerdict::Allowed`
// carries it — callers matching on the verdict need the type without reaching past
// the facade into `keymap-core`.
pub use keymap_core::{BreakReason, LegacyForm, RebindVerdict, validate_rebind};
// Opt-in terminal-survivability lint. Root re-export only (not prelude) — this is
// a diagnostic pass, not a per-event operation. Exposing it here closes the gap
// that previously forced a direct `keymap-core` dependency just for this function.
pub use keymap_core::{LegacyLint, legacy_lints};
// Command-palette prefix-completion index. Not in prelude — root re-export only
// (prelude membership deferred until showcase demonstrates mainstream use).
pub use keymap_core::cmd::CommandIndex;

pub use keymap_config::{BuildError, GLOBAL_LAYER, Warning, WarningKind};
pub use keymap_config::{to_toml, to_toml_layered, to_toml_layered_with_unbinds};
// Overlay-merge API. Not in prelude — advanced multi-config composition.
pub use keymap_config::{MergeNote, Merged, merge};

pub use keymap_seq::{Continuation, Match, SeqBindError, SequenceKeymap};
pub use keymap_seq::{PendingSequence, Step};
// Timeout-aware sequence buffer (the inter-key timer wrapper for PendingSequence).
pub use keymap_seq::{TimedPending, TimedStep};

// The crossterm adapter (`TryFrom<KeyEvent> for KeyInput`) lives in
// `keymap-core` behind its own `crossterm` feature; the impl is on the same
// `KeyInput` we already re-export, so enabling `keymap-suite/crossterm` makes
// `KeyInput::try_from(key_event)` resolve with no further glue. We re-export the
// one named type that conversion can fail with so the caller can match on it
// without reaching past the facade into `keymap-core`.
#[cfg(feature = "crossterm")]
pub use keymap_core::UnsupportedKey;

/// The result of loading a keymap configuration: named layers, a sequence
/// table, and any non-fatal warnings.
///
/// This is an alias for [`keymap_config::BuildOutput`] — the suite reuses the
/// underlying type rather than wrapping it, so users who later drop down to
/// `keymap-config` see the same value with no conversion. Field access
/// (`loaded.layers`, `loaded.sequences`, `loaded.warnings`) and the inherent
/// [`global`](Loaded::global) accessor are inherited.
///
/// Suite-only helpers (most notably [`LoadedExt::deny_warnings`] and
/// [`LoadedExt::pending_sequence`]) live on the [`LoadedExt`] trait, which the
/// [`prelude`] pulls in. If you write `use keymap_suite::Loaded;` directly
/// without the prelude (or without `use keymap_suite::LoadedExt;`), those
/// methods will be invisible — `rust-analyzer` will surface the trait as a
/// candidate, but the compile error reads as "no method named `deny_warnings`".
pub type Loaded<A> = keymap_config::BuildOutput<A>;

/// Suite-side conveniences attached to [`Loaded`].
///
/// These are placed on a trait — rather than as inherent methods — so they can
/// be defined here without owning the underlying type. The [`prelude`]
/// re-exports the trait, so a single `use keymap_suite::prelude::*;` is enough
/// to make `.deny_warnings()` and `.pending_sequence()` resolve.
pub trait LoadedExt: Sized {
    /// Returns the loaded keymap if no warnings were recorded, or hands the
    /// warnings back as an error so the caller can fail fast.
    ///
    /// This is the strict-mode escape hatch on a deliberately-lenient loader:
    /// the suite never elevates a [`Warning`] to a [`BuildError`] internally
    /// (that would diverge from `keymap-config`'s contract and make the same
    /// TOML behave differently depending on which entry point you used). When
    /// you do want to fail fast — typically at startup, in CI, or behind a
    /// "treat warnings as errors" flag — chain `deny_warnings()` onto the
    /// load.
    ///
    /// ```
    /// use keymap_suite::prelude::*;
    ///
    /// # #[derive(Clone, Debug, PartialEq)] enum Action {}
    /// let loaded = keymap_suite::from_toml_str("", |_| None::<Action>)?
    ///     .deny_warnings()
    ///     .expect("empty config has no warnings");
    /// # let _ = loaded;
    /// # Ok::<(), keymap_suite::BuildError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Err(warnings)` if the loader collected any non-fatal problem
    /// (chord conflict, unknown action, prefix shadow, …); the keymap is
    /// dropped in that case so the caller cannot accidentally proceed with a
    /// partially-bound table.
    fn deny_warnings(self) -> Result<Self, Vec<Warning>>;

    /// Returns a fresh, empty [`PendingSequence`] for feeding into this
    /// load's [`SequenceKeymap`].
    ///
    /// This is a discoverability shortcut: it returns
    /// [`PendingSequence::new`], which is what you would write directly. It is
    /// here so a caller reading `Loaded`'s methods finds the entry point
    /// without having to know that `PendingSequence` is the partner type.
    ///
    /// The buffer is held by the caller and fed [`SequenceKeymap`] per call
    /// (see [`PendingSequence::feed`]), so this does not borrow `self`.
    fn pending_sequence(&self) -> PendingSequence;
}

impl<A> LoadedExt for Loaded<A> {
    fn deny_warnings(self) -> Result<Self, Vec<Warning>> {
        if self.warnings.is_empty() {
            Ok(self)
        } else {
            Err(self.warnings)
        }
    }

    fn pending_sequence(&self) -> PendingSequence {
        PendingSequence::new()
    }
}

/// Either the configuration file could not be read, or its contents could not
/// be turned into a keymap.
///
/// I/O failures (file missing, permission denied) are kept distinct from
/// configuration errors (invalid TOML, unparseable key string), so a caller
/// can distinguish "the user hasn't created their config yet" from "the
/// config exists but is malformed".
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// The file could not be read from disk.
    Io(std::io::Error),
    /// The file was read but its contents could not be parsed into a keymap.
    Build(BuildError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(_) => f.write_str("failed to read keymap configuration file"),
            LoadError::Build(_) => f.write_str("failed to parse keymap configuration"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io(e) => Some(e),
            LoadError::Build(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        LoadError::Io(value)
    }
}

impl From<BuildError> for LoadError {
    fn from(value: BuildError) -> Self {
        LoadError::Build(value)
    }
}

/// Parses a TOML keymap configuration and resolves its action names with
/// `resolve`.
///
/// This is a thin convenience over [`keymap_config::from_str`], renamed
/// (`from_toml_str`) so it sits beside [`from_toml_path`] under one
/// vocabulary. The signature and semantics are identical to the underlying
/// function — including the lenient handling of conflicts and unknown
/// actions (see [`Warning`]).
///
/// # Errors
///
/// Returns a [`BuildError`] if the input is not valid TOML or contains a key
/// string that does not parse. Non-fatal problems (conflicts, unknown
/// actions, prefix shadows, …) are collected into
/// [`Loaded::warnings`](keymap_config::BuildOutput::warnings) so the rest of
/// the bindings still work.
pub fn from_toml_str<A, F>(toml: &str, resolve: F) -> Result<Loaded<A>, BuildError>
where
    F: FnMut(&str) -> Option<A>,
{
    keymap_config::from_str(toml, resolve)
}

/// Reads a TOML keymap configuration from `path` and resolves its action
/// names with `resolve`.
///
/// The file is read in full as UTF-8 — keymap configurations are small enough
/// that streaming the I/O is not worth the API complexity — and handed to
/// [`from_toml_str`].
///
/// # Errors
///
/// Returns [`LoadError::Io`] if the file cannot be read (missing, permission
/// denied, not UTF-8), and [`LoadError::Build`] if the contents are present
/// but malformed.
pub fn from_toml_path<A, P, F>(path: P, resolve: F) -> Result<Loaded<A>, LoadError>
where
    P: AsRef<Path>,
    F: FnMut(&str) -> Option<A>,
{
    let contents = std::fs::read_to_string(path.as_ref())?;
    let loaded = from_toml_str(&contents, resolve)?;
    Ok(loaded)
}

/// Every chord bound to `action` in one keymap layer — the reverse of
/// [`Keymap::get`], for help screens and which-key menus.
///
/// `Keymap` indexes chord → action; a "what keys run *this* action?" view
/// (a help screen, a command palette showing its shortcut) is the reverse, and
/// every such UI re-derives it from [`Keymap::iter`] with the same
/// filter-by-action boilerplate. This is that one line, once.
///
/// It returns borrowed [`KeyInput`]s in **unspecified order** and does not
/// format them: rendering is the caller's (`.to_string()` for the canonical
/// chord string, then sort/dedup to taste), so the suite imposes no display or
/// ordering policy. A chord can appear once per binding, so multiple keys bound
/// to the same action all come back.
///
/// This works on **one** layer. With a layered chain, map it over your active
/// layers — the suite does not fold the chain for you, because which layers are
/// active (and in what order) is your application state, exactly as it is for
/// [`resolve_layered`].
///
/// ```
/// use keymap_suite::prelude::*;
///
/// # #[derive(Clone, Debug, PartialEq)] enum Action { Save, Quit }
/// let mut map = Keymap::new();
/// map.bind(KeyInput::new(Key::Char('s'), Modifiers::CTRL), Action::Save);
/// map.bind(KeyInput::new(Key::Char('x'), Modifiers::CTRL), Action::Save);
///
/// let mut keys: Vec<String> = keys_for_action(&map, &Action::Save)
///     .iter()
///     .map(ToString::to_string)
///     .collect();
/// keys.sort(); // order is unspecified, so the caller sorts for display
/// assert_eq!(keys, ["ctrl+s", "ctrl+x"]);
///
/// assert!(keys_for_action(&map, &Action::Quit).is_empty());
/// ```
pub fn keys_for_action<'a, A>(keymap: &'a Keymap<A>, action: &A) -> Vec<&'a KeyInput>
where
    A: PartialEq,
{
    keymap
        .iter()
        .filter_map(|(input, bound)| (bound == action).then_some(input))
        .collect()
}

/// Terse constructors for the common "modifier + character" chords, so building
/// a [`KeyInput`] in code reads `ctrl('s')` rather than
/// `KeyInput::new(Key::Char('s'), Modifiers::CTRL)`.
///
/// These are the shapes that show up over and over in tests and in apps that
/// hard-code a few default bindings. They cover exactly the
/// **single-modifier-on-a-character** case; everything else has a better tool:
///
/// - **Named keys** (`Tab`, `F1`, `Enter`, arrows) and **multi-modifier chords**
///   (`ctrl+shift+s`): parse the canonical string, which handles them all
///   uniformly — `"ctrl+shift+f1".parse::<KeyInput>()` — or reach for
///   [`KeyInput::new`] with the explicit [`Key`]/[`Modifiers`].
/// - **A bare `shift` + a character** is deliberately *absent*: a sole Shift on a
///   character folds away (it is redundant with the resolved glyph — see
///   [`KeyInput`]), so `shift('a')` would normalize to exactly `key('a')` and
///   only mislead. Shift survives only on non-character keys (`shift+tab`) or
///   alongside another modifier (`ctrl+shift+s`), both of which go through
///   `.parse()` / `KeyInput::new`.
///
/// Every constructor here funnels through [`KeyInput::normalized`] — the same
/// rule the TOML grammar and the crossterm adapter use — so a chord you build
/// with `ctrl('s')` is byte-for-byte the chord that arrives at runtime, and
/// lookup cannot silently miss.
///
/// The names are short and common (`key`, `ctrl`, `alt`), so they live in this
/// module rather than the [`prelude`]'s flat namespace. Pull them in where you
/// want them — typically the top of a test module:
///
/// ```
/// use keymap_suite::chords::{ctrl, key};
///
/// assert_eq!(ctrl('s'), "ctrl+s".parse().unwrap());
/// assert_eq!(key('q'), "q".parse().unwrap());
/// ```
pub mod chords {
    use crate::{Key, KeyInput, Modifiers};

    /// A character with no modifiers (e.g. `key('q')` → `q`).
    #[must_use]
    pub fn key(c: char) -> KeyInput {
        KeyInput::normalized(Key::Char(c), Modifiers::NONE)
    }

    /// A character held with Control (e.g. `ctrl('s')` → `ctrl+s`).
    #[must_use]
    pub fn ctrl(c: char) -> KeyInput {
        KeyInput::normalized(Key::Char(c), Modifiers::CTRL)
    }

    /// A character held with Alt/Option (e.g. `alt('x')` → `alt+x`).
    #[must_use]
    pub fn alt(c: char) -> KeyInput {
        KeyInput::normalized(Key::Char(c), Modifiers::ALT)
    }
}

/// The one-import bundle most callers want: the vocabulary, the resolver, the
/// sequence buffer, and the load helpers, in one `use`.
///
/// ```
/// use keymap_suite::prelude::*;
/// ```
///
/// What is in here is the answer to "what do nine out of ten TUI authors
/// import?". Niche items (the legacy-survivability lint, the byte decoder,
/// the raw-byte passthrough variants) are reachable from the underlying
/// crates but are deliberately *not* in the prelude — keeping it small keeps
/// the `use *;` discipline cheap.
pub mod prelude {
    // Vocabulary — the chord type and its parts.
    pub use crate::{Key, KeyInput, Modifiers};
    // Tables and resolution — single-chord lookup with layered context.
    pub use crate::{Keymap, resolve_layered};
    // Sequence resolution — multi-key sequences (`ctrl+x ctrl+s`, leader keys).
    // `TimedPending`/`TimedStep` are the timeout-aware wrappers for callers who
    // need an inter-key timer; `PendingSequence`/`Step` remain for callers who
    // drive the timer themselves.
    pub use crate::{Match, PendingSequence, SequenceKeymap, Step};
    pub use crate::{TimedPending, TimedStep};
    // Configuration result and its helpers. `WarningKind` gives callers a
    // category tag without pattern-matching on the full Warning variant.
    pub use crate::{Loaded, LoadedExt, Warning, WarningKind};
    // Discovery — the reverse of resolution, for help screens / which-key menus.
    pub use crate::keys_for_action;
    // Errors.
    pub use crate::{BuildError, LoadError};
    // Terse chord constructors, kept namespaced (`chords::ctrl('s')`) because
    // `key`/`ctrl`/`alt` are short, collision-prone names.
    pub use crate::chords;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Action {
        Quit,
        Save,
        SplitPane,
    }

    fn resolve(name: &str) -> Option<Action> {
        match name {
            "quit" => Some(Action::Quit),
            "save" => Some(Action::Save),
            "split_pane" => Some(Action::SplitPane),
            _ => None,
        }
    }

    #[test]
    fn from_toml_str_returns_layers_and_warnings() {
        let toml = r#"
[keys]
"ctrl+q" = "quit"
"ctrl+s" = "save"
"control+s" = "split_pane"
"ctrl+z" = "undo"

[layers.panel]
"ctrl+s" = "split_pane"
"#;
        let out = from_toml_str(toml, resolve).expect("valid TOML");
        // The global layer is always present.
        assert!(out.layers.contains_key(GLOBAL_LAYER));
        assert!(out.layers.contains_key("panel"));
        // Two warnings: the within-layer conflict on ctrl+s, and the unknown action.
        assert_eq!(out.warnings.len(), 2);
    }

    #[test]
    fn deny_warnings_returns_self_when_clean_and_warnings_otherwise() {
        // Clean: empty config, no warnings.
        let clean: Loaded<Action> = from_toml_str("", |_| None).expect("empty config parses");
        assert!(clean.warnings.is_empty());
        let _kept: Loaded<Action> = clean.deny_warnings().expect("no warnings → Ok");

        // Dirty: an unknown action name yields a Warning::UnknownAction.
        let dirty_toml = r#"
[keys]
"ctrl+q" = "no_such_action"
"#;
        let dirty = from_toml_str(dirty_toml, resolve).expect("valid TOML");
        let returned = dirty.deny_warnings().expect_err("warnings → Err(warnings)");
        assert_eq!(returned.len(), 1);
        assert!(matches!(returned[0], Warning::UnknownAction { .. }));
    }

    #[test]
    fn pending_sequence_method_is_a_discoverability_shortcut_for_new() {
        let loaded: Loaded<Action> = from_toml_str("", |_| None).expect("empty config parses");
        let via_method = loaded.pending_sequence();
        let via_new = PendingSequence::new();
        assert_eq!(via_method.is_empty(), via_new.is_empty());
        assert!(via_method.pending().is_empty());
    }

    #[test]
    fn load_error_distinguishes_io_from_build() {
        // I/O: a path that does not exist.
        let err = from_toml_path::<Action, _, _>("/no/such/keymap.toml", |_| None)
            .expect_err("missing file → Err");
        assert!(matches!(err, LoadError::Io(_)));
    }

    #[test]
    fn end_to_end_resolve_layered_via_re_exports() {
        // The shape of a minimal "TUI loop": load TOML, pick a layer chain,
        // resolve one key. All names reached from `prelude::*`.
        use crate::prelude::*;

        let toml = r#"
[keys]
"ctrl+q" = "quit"

[layers.panel]
"ctrl+s" = "split_pane"
"#;
        let loaded = from_toml_str(toml, resolve).expect("valid TOML");
        let key_q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);

        let chain = [loaded.global()];
        assert_eq!(
            resolve_layered(chain.iter().copied(), &key_q),
            Some(&Action::Quit),
        );

        // Panel layer beats the global on a chord both layers bind.
        let panel = &loaded.layers["panel"];
        let key_s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        let chain = [panel, loaded.global()];
        assert_eq!(
            resolve_layered(chain.iter().copied(), &key_s),
            Some(&Action::SplitPane),
        );
    }

    #[test]
    fn keys_for_action_returns_every_chord_bound_to_it() {
        let toml = r#"
[keys]
"ctrl+s" = "save"
"ctrl+w" = "save"
"ctrl+q" = "quit"
"#;
        let loaded = from_toml_str(toml, resolve).expect("valid TOML");
        let mut keys: Vec<String> = keys_for_action(loaded.global(), &Action::Save)
            .iter()
            .map(ToString::to_string)
            .collect();
        keys.sort();
        assert_eq!(keys, ["ctrl+s", "ctrl+w"]);

        // An action bound nowhere in this layer comes back empty.
        assert!(keys_for_action(loaded.global(), &Action::SplitPane).is_empty());
    }

    #[test]
    fn keys_for_action_round_trips_through_get() {
        // Theresa's invariant: every chord keys_for_action hands back must
        // resolve, via Keymap::get, to the very action we asked about. This is
        // what keeps a help screen honest — no "shown but does nothing" chord.
        let toml = r#"
[keys]
"ctrl+s" = "save"
"ctrl+w" = "save"
"ctrl+q" = "quit"
"g" = "quit"
"#;
        let loaded = from_toml_str(toml, resolve).expect("valid TOML");
        for action in [Action::Quit, Action::Save, Action::SplitPane] {
            for chord in keys_for_action(loaded.global(), &action) {
                assert_eq!(
                    loaded.global().get(chord),
                    Some(&action),
                    "chord {chord} listed for {action:?} but resolves elsewhere",
                );
            }
        }
    }

    #[test]
    fn chord_constructors_match_their_parsed_equivalents() {
        use crate::chords::{alt, ctrl, key};

        // The terse constructor and the canonical-string parse must agree — this
        // is what lets a binding built in code and one loaded from TOML collide.
        assert_eq!(key('q'), "q".parse().unwrap());
        assert_eq!(ctrl('s'), "ctrl+s".parse().unwrap());
        assert_eq!(alt('x'), "alt+x".parse().unwrap());

        // And they match what the explicit constructor produces.
        assert_eq!(ctrl('s'), KeyInput::new(Key::Char('s'), Modifiers::CTRL));
    }

    #[test]
    fn chord_constructors_funnel_through_normalization() {
        use crate::chords::{ctrl, key};

        // `ctrl+shift+s` keeps Shift (a second modifier is held), so it is a
        // distinct chord — the constructors are not a way to sneak past
        // normalization. `key('a')` is bare, with no Shift to fold.
        assert_eq!(key('a'), KeyInput::new(Key::Char('a'), Modifiers::NONE));
        assert_ne!(ctrl('s'), KeyInput::new(Key::Char('s'), Modifiers::NONE));
    }

    // ─── S1-S5 re-export smoke tests ─────────────────────────────────────────
    //
    // Each test reaches the new API exclusively through `keymap_suite` names to
    // confirm the facade wires everything up. Correctness is tested in the source
    // crates; here we just check that the types and functions are accessible.

    #[test]
    fn command_index_is_reachable_from_suite_root() {
        // `CommandIndex` is root-only (not in prelude by design); reached via
        // `super::CommandIndex` which mirrors `keymap_suite::CommandIndex` for
        // consumers of the published crate.
        let mut index: CommandIndex<Action> = CommandIndex::new();
        index.bind("quit", Action::Quit);
        index.bind("save", Action::Save);

        assert_eq!(index.get("quit"), Some(&Action::Quit));
        assert_eq!(index.get("nope"), None);

        let completions: Vec<&str> = index.complete("s").map(|(name, _)| name).collect();
        assert_eq!(completions, ["save"]);
    }

    #[test]
    fn timed_pending_is_reachable_from_suite_prelude() {
        use crate::prelude::*;
        use std::time::{Duration, Instant};

        let loaded = from_toml_str(
            "[[sequences]]\nkeys = [\"g\", \"g\"]\naction = \"quit\"\n",
            resolve,
        )
        .expect("valid TOML");

        let mut pending = TimedPending::new();
        let now = Instant::now();
        let window = Duration::from_millis(500);
        let g = KeyInput::new(Key::Char('g'), Modifiers::NONE);

        // First `g` press: enters a prefix.
        let step = pending.feed(&loaded.sequences, g, now, window);
        assert!(step.expired.is_none(), "no expiry on first press");
        // (step.step is Prefix or Passthrough depending on map; just checking it doesn't panic)
        let _ = step.step;
    }

    #[test]
    fn validate_rebind_is_reachable_from_suite_root() {
        let mut layer: Keymap<Action> = Keymap::new();
        layer.bind(KeyInput::new(Key::Char('q'), Modifiers::CTRL), Action::Quit);
        let layers = [&layer];
        let proposed = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        let reserved = [KeyInput::new(Key::Char('q'), Modifiers::CTRL)];

        // `ctrl+s` is not reserved — should be Allowed.
        let verdict = validate_rebind(&layers, 0, proposed, &reserved);
        assert!(
            matches!(verdict, RebindVerdict::Allowed { .. }),
            "ctrl+s is not reserved: {verdict:?}"
        );

        // `ctrl+q` is reserved — should break.
        let verdict = validate_rebind(&layers, 0, reserved[0], &reserved);
        assert!(
            matches!(
                verdict,
                RebindVerdict::BreaksReserved {
                    reason: BreakReason::DirectSteal,
                    ..
                }
            ),
            "ctrl+q is reserved: {verdict:?}"
        );
    }

    #[test]
    fn merge_is_reachable_from_suite_root() {
        let base = from_toml_str("[keys]\n\"ctrl+s\" = \"save\"\n", resolve).unwrap();
        let overlay = from_toml_str("[keys]\n\"ctrl+s\" = \"quit\"\n", resolve).unwrap();
        let merged = merge(base, overlay);

        let ctrl_s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(merged.output.global().get(&ctrl_s), Some(&Action::Quit));
        assert!(
            merged
                .notes
                .iter()
                .any(|n| matches!(n, MergeNote::Overrode { .. }))
        );
    }

    #[test]
    fn warning_kind_is_reachable_from_suite_prelude() {
        use crate::prelude::*;

        let toml = "[keys]\n\"ctrl+z\" = \"unknown_action\"\n";
        let loaded = from_toml_str(toml, resolve).unwrap();
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(loaded.warnings[0].kind(), WarningKind::UnknownAction);
    }

    #[test]
    fn loaded_unbinds_field_is_visible_via_suite() {
        // `BuildOutput::unbinds` was added in S4; `Loaded` is a type alias for
        // `BuildOutput` so the field is automatically accessible — this test pins it.
        let toml = "[keys]\n\"ctrl+s\" = false\n";
        let loaded = from_toml_str(toml, resolve).unwrap();
        assert!(
            !loaded.unbinds.is_empty(),
            "unbinds field visible through Loaded alias"
        );
    }

    #[test]
    fn legacy_lints_is_reachable_from_suite_root() {
        // `legacy_lints` and `LegacyLint` are root-only (not in prelude —
        // opt-in diagnostic, not a per-event operation). Verify the re-export
        // is wired correctly and the function runs without requiring a direct
        // `keymap-core` dependency.
        let toml = "[keys]\n\"ctrl+s\" = \"save\"\n";
        let loaded = from_toml_str(toml, resolve).unwrap();
        let global = loaded.global();

        // `legacy_lints` returns a Vec<LegacyLint> — verify it does not panic
        // and the result is of the right type.
        let lints: Vec<LegacyLint> = legacy_lints(global);
        // ctrl+s is a safe chord (C0 representable); expect no lints for it.
        assert!(
            lints.is_empty(),
            "ctrl+s alone should produce no legacy lints: {lints:?}"
        );
    }

    #[cfg(feature = "crossterm")]
    #[test]
    fn crossterm_key_event_converts_through_the_facade() {
        // With the `crossterm` feature on, `KeyInput::try_from(KeyEvent)` is
        // reachable using only names from the suite (KeyInput re-exported here,
        // KeyEvent from the crossterm crate), and a key with no neutral form
        // fails with the re-exported `UnsupportedKey` rather than panicking.
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let ev = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let input = KeyInput::try_from(ev).expect("ctrl+s converts");
        assert_eq!(input, KeyInput::new(Key::Char('s'), Modifiers::CTRL));

        let unmappable = KeyEvent::new(KeyCode::CapsLock, KeyModifiers::empty());
        let err: Result<KeyInput, UnsupportedKey> = KeyInput::try_from(unmappable);
        assert!(err.is_err());
    }
}
