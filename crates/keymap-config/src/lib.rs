//! # keymap-config
//!
//! Builds a [`Keymap`] from a TOML `[keys]` table, resolving action names with a
//! caller-supplied function and reporting problems that should not silently
//! pass.
//!
//! ```
//! #[derive(Clone, Debug, PartialEq)]
//! enum Action { Quit, Save }
//!
//! let toml = r#"
//! [keys]
//! "ctrl+q" = "quit"
//! "ctrl+s" = "save"
//! "#;
//!
//! let out = keymap_config::from_str(toml, |name| match name {
//!     "quit" => Some(Action::Quit),
//!     "save" => Some(Action::Save),
//!     _ => None,
//! })
//! .unwrap();
//!
//! assert!(out.warnings.is_empty());
//! ```
//!
//! ## Named layers
//!
//! Bindings live in *layers*. The bare top-level `[keys]` table is the
//! [`GLOBAL_LAYER`] layer; additional `[layers.<name>]` tables each build a layer
//! of that name, holding chord→action entries directly:
//!
//! ```toml
//! [keys]               # the implicit "global" layer
//! "ctrl+q" = "quit"
//!
//! [layers.panel]       # a caller-named layer
//! "ctrl+s" = "split"
//! ```
//!
//! [`from_str`] returns these as [`BuildOutput::layers`], a name→[`Keymap`] map
//! always containing `"global"`. Which layers are active, and in what order, is
//! **not** this crate's concern: the caller picks them per event and resolves
//! with [`keymap_core::resolve_layered`] (first layer to bind a chord wins, misses
//! fall through). The same chord in two layers is therefore an *override*, not a
//! conflict — this crate only ever reports conflicts *within* a single layer.
//! Layer names are opaque labels; the library attaches meaning to none of them.
//!
//! ## What is an error versus a warning
//!
//! Structural problems — malformed TOML, or a key string that does not parse —
//! are [`BuildError`]: there is no usable map to return. Survivable problems —
//! two keys resolving to the same chord, or an action name the resolver does not
//! recognize — are collected into [`BuildOutput::warnings`] so the rest of the
//! bindings still work and the user can be told what to fix.
//!
//! ## Legacy-terminal survivability (opt-in, separate from warnings)
//!
//! "This binding won't survive a legacy terminal" (e.g. a `cmd+…` chord a legacy
//! terminal cannot deliver, or `ctrl+i` which is indistinguishable from `tab`) is
//! deliberately *not* a [`Warning`]: it depends on the deployment terminal, not
//! the config's correctness, and folding it into [`BuildOutput::warnings`] would
//! make a perfectly good config report warnings. It is exposed instead as the
//! opt-in [`keymap_core::legacy_lints`], which you call on a built keymap when
//! you want it: `keymap_core::legacy_lints(out.global())` (or on any one layer).
//! Callers gating on `warnings.is_empty()` are unaffected.
//!
//! ## Multi-key sequences
//!
//! Alongside the single-chord `[keys]` table, an `[[sequences]]` array of tables
//! binds *sequences* of chords (leader trees, `ctrl+x ctrl+s`) into a
//! [`SequenceKeymap`](keymap_seq::SequenceKeymap), returned as
//! [`BuildOutput::sequences`]:
//!
//! ```toml
//! [keys]
//! "ctrl+q" = "quit"
//!
//! [[sequences]]
//! keys = ["ctrl+x", "ctrl+s"]
//! action = "save"
//! ```
//!
//! Each element of `keys` reuses the single-chord grammar, so no new key syntax
//! is introduced. Two sequence bindings that share a prefix cannot coexist
//! without a timeout (see `keymap-seq`); such a pair is reported as
//! [`Warning::PrefixShadow`] and the later one is dropped. A single-key binding
//! that shadows a sequence (e.g. `"j"` in `[keys]` versus a sequence starting
//! with `j`) is reported as the advisory [`Warning::SequenceShadow`] — both
//! bindings are kept (they live in separate maps), so it only flags that the
//! caller composing the two maps must resolve the overlap with a timeout.
//!
//! ## Runnable examples
//!
//! Under [`examples/`](https://github.com/S-Nakamur-a/keymap-rs/tree/main/crates/keymap-config/examples):
//! `cargo run -p keymap-config --example load_config` walks the TOML-to-keymap
//! pipeline end to end, and `--example rebind` shows the runtime-rebind flow
//! on top of `keymap_term::decode`.

use std::collections::{BTreeMap, HashMap};

use keymap_core::{KeyInput, Keymap, ParseKeyInputError};
use keymap_seq::{SeqBindError, SequenceKeymap};
use serde::Deserialize;

/// The name of the layer built from the top-level `[keys]` table.
///
/// It is the one name this crate assigns: the bare `[keys]` table (and an
/// explicit `[layers.global]`, if present) build the layer under this key, which
/// is **always present** in [`BuildOutput::layers`] even when empty. Every other
/// layer name is opaque to the library — it is just the label the config author
/// chose; the library attaches no meaning to it and never decides when a layer is
/// active. That selection stays caller-side (see [`keymap_core::resolve_layered`]).
pub const GLOBAL_LAYER: &str = "global";

/// The result of a successful build: the named layers plus any non-fatal warnings.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BuildOutput<A> {
    /// The assembled layers, keyed by name. The top-level `[keys]` table builds
    /// the [`GLOBAL_LAYER`] layer (always present, even when empty); each
    /// `[layers.<name>]` table builds the layer of that name. Within any one
    /// layer, conflicting chords resolve to the last binding. The same chord
    /// bound in two *different* layers is not a conflict — it is the override the
    /// caller composes with [`keymap_core::resolve_layered`], so this crate never
    /// reports across layer boundaries.
    ///
    /// Always contains the `"global"` key, so `layers.len()` counts the global
    /// layer even when the config bound nothing in it.
    pub layers: BTreeMap<String, Keymap<A>>,
    /// The assembled sequence map, built from the top-level `[[sequences]]`
    /// tables. Sequences are **not** layered (they belong to the global config);
    /// empty when the config has no sequences.
    pub sequences: SequenceKeymap<A>,
    /// Problems that did not prevent building (conflicts, unknown actions,
    /// prefix shadows), in the order they were first seen.
    pub warnings: Vec<Warning>,
    /// Chords declared as tombstones (`= false`) in the config, grouped by layer
    /// name. A tombstone records an *intent to remove* a chord from a base
    /// keymap; the chord is not present in `layers` (it has no action), but is
    /// carried here so [`merge`] can apply the removal to a base
    /// [`BuildOutput`].
    ///
    /// Empty when the config has no `= false` declarations.
    pub unbinds: BTreeMap<String, Vec<KeyInput>>,
}

impl<A> BuildOutput<A> {
    /// The [`GLOBAL_LAYER`] layer (built from the top-level `[keys]` table).
    ///
    /// This is the convenience accessor for the common case of a single,
    /// unlayered keymap: `out.global()` is exactly `&out.layers[GLOBAL_LAYER]`.
    /// A config with no `[keys]` table yields an empty global layer, not an
    /// absent one.
    ///
    /// # Panics
    ///
    /// Never in practice: [`from_str`] always inserts the global layer (empty if
    /// the config had no `[keys]` table), so the lookup cannot miss.
    #[must_use]
    pub fn global(&self) -> &Keymap<A> {
        self.layers
            .get(GLOBAL_LAYER)
            .expect("the global layer is always inserted")
    }
}

/// A non-fatal problem found while building a [`Keymap`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Warning {
    /// Two or more keys resolved to the same chord *within one layer*; the last
    /// one wins. A named layer's conflict uses this same variant and carries no
    /// layer name, so two layers each conflicting on the same chord produce
    /// warnings indistinguishable on their own (the chord identifies the clash,
    /// not the layer). The same chord across *different* layers is an override,
    /// not a conflict, and is never reported.
    Conflict {
        /// The shared chord, in canonical form (e.g. `"ctrl+a"`).
        chord: String,
        /// The competing action names, in file order.
        contenders: Vec<String>,
        /// The action name that won (the last binding for the chord).
        winner: String,
    },
    /// An action name in the config was not recognized by the resolver; the
    /// binding was skipped.
    UnknownAction {
        /// The key string as written in the config. For a sequence, the
        /// canonical chords joined by spaces.
        key: String,
        /// The unrecognized action name.
        action: String,
    },
    /// Two sequence bindings share a prefix: the shorter (`prefix`) is a proper
    /// prefix of the longer (`shadowed`), so they cannot coexist without a
    /// timeout to tell them apart. The later-in-file binding was dropped to keep
    /// the sequence table prefix-free.
    PrefixShadow {
        /// The shorter sequence, one canonical chord per element.
        prefix: Vec<String>,
        /// The action bound to the shorter sequence.
        prefix_action: String,
        /// The longer sequence that shares the prefix, one chord per element.
        shadowed: Vec<String>,
        /// The action bound to the longer sequence.
        shadowed_action: String,
    },
    /// A `[[sequences]]` entry had an empty `keys` array; it was skipped.
    EmptySequence {
        /// The action name the empty sequence would have been bound to.
        action: String,
    },
    /// A single-chord `[keys]` binding collides with the first key of a
    /// `[[sequences]]` entry (e.g. `"j"` in `[keys]` and a sequence `["j", "k"]`).
    /// Pressing that chord is then ambiguous — fire the single action now, or wait
    /// to see if the sequence continues? — and can only be disambiguated by a
    /// caller-side timeout (the vim `jj` case; see `keymap-seq`).
    ///
    /// Unlike [`PrefixShadow`](Warning::PrefixShadow), **nothing is dropped**: the
    /// chord stays in its layer (the global one — this is checked only against the
    /// global layer) and the sequence stays in [`BuildOutput::sequences`] (they are
    /// separate maps). This is purely an
    /// advisory that the caller composing the two maps must resolve the overlap.
    SequenceShadow {
        /// The single chord, in canonical form (e.g. `"j"`).
        chord: String,
        /// The action bound to the single chord.
        chord_action: String,
        /// One sequence the chord shadows — the lexicographically-first when it
        /// shadows several — one canonical chord per element (its first equals
        /// `chord`).
        sequence: Vec<String>,
        /// The action bound to that sequence.
        sequence_action: String,
    },
}

/// A category tag for a [`Warning`], without the field data.
///
/// Useful for filtering or routing warnings without pattern-matching on the full
/// variant: `warning.kind() == WarningKind::Conflict` tests the category in one
/// comparison, even when you do not need the chord or contender details.
///
/// This is `#[non_exhaustive]` — new [`Warning`] variants will add corresponding
/// `WarningKind` variants in a non-breaking additive release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WarningKind {
    /// See [`Warning::Conflict`].
    Conflict,
    /// See [`Warning::UnknownAction`].
    UnknownAction,
    /// See [`Warning::PrefixShadow`].
    PrefixShadow,
    /// See [`Warning::EmptySequence`].
    EmptySequence,
    /// See [`Warning::SequenceShadow`].
    SequenceShadow,
}

impl Warning {
    /// Returns the category of this warning without the field data.
    ///
    /// Useful for filtering: `w.kind() == WarningKind::Conflict` tests the
    /// category in a single comparison, even when the field details are not needed.
    #[must_use]
    pub fn kind(&self) -> WarningKind {
        match self {
            Warning::Conflict { .. } => WarningKind::Conflict,
            Warning::UnknownAction { .. } => WarningKind::UnknownAction,
            Warning::PrefixShadow { .. } => WarningKind::PrefixShadow,
            Warning::EmptySequence { .. } => WarningKind::EmptySequence,
            Warning::SequenceShadow { .. } => WarningKind::SequenceShadow,
            // Non-exhaustive: future variants covered at compile time.
        }
    }
}

impl core::fmt::Display for Warning {
    /// Formats the warning as a single human-readable line.
    ///
    /// **Display only — do not parse this output.** The format is not stable and
    /// may change between minor versions; it is intended for logging and user
    /// messages, not machine consumption.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Warning::Conflict { chord, winner, .. } => {
                write!(f, "conflict on {chord:?}: {winner:?} wins")
            }
            Warning::UnknownAction { key, action } => {
                write!(f, "unknown action {action:?} for key {key:?}")
            }
            Warning::PrefixShadow {
                prefix,
                shadowed,
                prefix_action,
                ..
            } => {
                write!(
                    f,
                    "prefix shadow: {:?} ({prefix_action:?}) shadows {:?}; later binding dropped",
                    prefix.join(" "),
                    shadowed.join(" ")
                )
            }
            Warning::EmptySequence { action } => {
                write!(f, "empty sequence for action {action:?}")
            }
            Warning::SequenceShadow {
                chord,
                sequence,
                chord_action,
                ..
            } => {
                write!(
                    f,
                    "sequence shadow: chord {chord:?} ({chord_action:?}) shadows sequence {:?}",
                    sequence.join(" ")
                )
            }
        }
    }
}

/// A fatal problem that prevents building a [`Keymap`].
#[derive(Debug)]
#[non_exhaustive]
pub enum BuildError {
    /// The input was not valid TOML.
    Toml(toml::de::Error),
    /// A key string in the `[keys]` table could not be parsed.
    KeyParse {
        /// The offending key string as written.
        key: String,
        /// The underlying parse error.
        source: ParseKeyInputError,
    },
    /// A chord in a `[keys]` or `[layers.<name>]` table was assigned `= true`.
    ///
    /// Only `= false` is a valid tombstone (unbind declaration); `= true` is
    /// rejected as an explicit error to prevent accidental partial unbinds. In
    /// 0.1.0 both `= true` and `= false` caused `BuildError::Toml` (type
    /// mismatch); 0.1.1 accepts `= false` but keeps `= true` fatal.
    InvalidTombstone {
        /// The offending chord string as written.
        key: String,
    },
}

impl core::fmt::Display for BuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BuildError::Toml(_) => f.write_str("invalid TOML"),
            BuildError::KeyParse { key, .. } => write!(f, "invalid key string {key:?}"),
            BuildError::InvalidTombstone { key } => {
                write!(
                    f,
                    "invalid tombstone for {key:?}: use `= false` to unbind, not `= true`"
                )
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BuildError::Toml(e) => Some(e),
            BuildError::KeyParse { source, .. } => Some(source),
            BuildError::InvalidTombstone { .. } => None,
        }
    }
}

impl From<toml::de::Error> for BuildError {
    fn from(e: toml::de::Error) -> Self {
        BuildError::Toml(e)
    }
}

// `deny_unknown_fields` makes a misspelled or unsupported key a `BuildError::Toml`
// rather than a silent no-op — chosen pre-1.0 because adding it *later* would
// reject configs that used to parse. It also keeps the door open for additive
// fields (e.g. a future per-`[[sequences]]` `layer = "..."` tag) without their
// typos failing silently.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    keys: BTreeMap<String, RawValue>,
    #[serde(default)]
    layers: BTreeMap<String, BTreeMap<String, RawValue>>,
    #[serde(default)]
    sequences: Vec<RawSequence>,
}

/// A chord's value in TOML: either an action name (string) or a tombstone
/// (`false` = unbind, `true` = error).
///
/// In 0.1.0 any non-string chord value was a `BuildError::Toml`. In 0.1.1
/// `= false` is newly accepted as a tombstone (unbind). `= true` remains a
/// `BuildError::InvalidTombstone` — an explicit error on `true` prevents
/// accidental partial unbinds from a copy-paste of `= true` (i.e. whatever
/// the caller meant was not "bind this chord to nothing").
#[derive(Deserialize)]
#[serde(untagged)]
enum RawValue {
    /// A chord binding: `"chord" = "action_name"`.
    Action(String),
    /// A chord tombstone: `"chord" = false` (unbind) or `"chord" = true` (error).
    Bool(bool),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSequence {
    keys: Vec<String>,
    action: String,
}

/// The built sequence map paired with its sequence→action-name dictionary (the
/// names let a cross-shadow be reported against the single-key table).
type SequenceBuild<A> = (SequenceKeymap<A>, HashMap<Vec<KeyInput>, String>);

/// Return type of [`build_layer`]: the keymap, the winning action-name-per-chord
/// dictionary (for cross-shadow detection in the global layer), and the list of
/// tombstone chords (`= false`).
type LayerBuild<A> = (Keymap<A>, HashMap<KeyInput, String>, Vec<KeyInput>);

/// Builds a [`Keymap`] from a TOML string.
///
/// The `[keys]` table maps key strings (see `keymap_core::KeyInput`'s `FromStr`)
/// to action names; `resolve` turns each action name into the caller's action
/// type `A`, returning `None` for names it does not recognize.
///
/// # Errors
///
/// Returns [`BuildError`] if the input is not valid TOML, or if any key string
/// fails to parse. Unknown action names and chord conflicts are not errors —
/// they are returned in [`BuildOutput::warnings`].
pub fn from_str<A, F>(toml_str: &str, mut resolve: F) -> Result<BuildOutput<A>, BuildError>
where
    F: FnMut(&str) -> Option<A>,
{
    let RawConfig {
        keys,
        mut layers,
        sequences: raw_sequences,
    } = toml::from_str(toml_str)?;

    let mut warnings = Vec::new();
    let mut built: BTreeMap<String, Keymap<A>> = BTreeMap::new();
    let mut unbinds: BTreeMap<String, Vec<KeyInput>> = BTreeMap::new();

    // The global layer is the top-level `[keys]` table plus an explicit
    // `[layers.global]` if the author also wrote one. The two are concatenated —
    // `[keys]` first, then `[layers.global]` — and fed through the same per-chord
    // pipeline, so a chord present in both is reported as an ordinary in-layer
    // `Conflict` (and, being last, the `[layers.global]` entry wins) rather than
    // silently dropped. The global layer is always inserted, even when empty.
    let explicit_global = layers.remove(GLOBAL_LAYER).unwrap_or_default();
    let global_entries = keys.into_iter().chain(explicit_global);
    let (global_keymap, global_names, global_unbinds) =
        build_layer(global_entries, &mut resolve, &mut warnings)?;
    built.insert(GLOBAL_LAYER.to_string(), global_keymap);
    if !global_unbinds.is_empty() {
        unbinds.insert(GLOBAL_LAYER.to_string(), global_unbinds);
    }

    // Every other named layer, in `BTreeMap` name order so warnings are
    // deterministic. Each layer's conflicts are detected within itself; layer
    // boundaries are never crossed (the caller owns the active chain).
    for (name, raw_keys) in layers {
        // Only the global layer feeds the cross-shadow check below, so a
        // non-global layer's name dictionary is computed but not needed here.
        let (keymap, _names, layer_unbinds) = build_layer(raw_keys, &mut resolve, &mut warnings)?;
        built.insert(name.clone(), keymap);
        if !layer_unbinds.is_empty() {
            unbinds.insert(name, layer_unbinds);
        }
    }

    // Sequences are global-only, so the cross-shadow check (single chord versus
    // the first key of a sequence) runs against the global layer alone.
    let (sequences, seq_names) = build_sequences(raw_sequences, &mut resolve, &mut warnings)?;
    detect_cross_shadows(&global_names, &seq_names, &mut warnings);

    Ok(BuildOutput {
        layers: built,
        sequences,
        warnings,
        unbinds,
    })
}

/// Builds one layer's [`Keymap`] from its `(raw key string, raw value)`
/// entries, appending survivable problems to `warnings`.
///
/// Entries are grouped by the chord they normalize to (so different spellings of
/// the same chord — `ctrl+a` and `control+a` — collide), visited in first-seen
/// order. Within a group the last resolvable binding wins and a [`Warning::Conflict`]
/// is reported; an unresolvable action name is a [`Warning::UnknownAction`] and
/// that single entry is skipped. A `= false` tombstone is collected into the
/// returned unbind list without adding a binding; a `= true` tombstone is a fatal
/// [`BuildError::InvalidTombstone`].
///
/// Returns `(keymap, names, unbinds)`:
/// - `keymap`: the resolved bindings.
/// - `names`: the winning action name per chord (used by the cross-shadow check
///   in the global layer; other callers discard it).
/// - `unbinds`: chords declared with `= false`, in parse order.
fn build_layer<A, I, F>(
    entries: I,
    resolve: &mut F,
    warnings: &mut Vec<Warning>,
) -> Result<LayerBuild<A>, BuildError>
where
    I: IntoIterator<Item = (String, RawValue)>,
    F: FnMut(&str) -> Option<A>,
{
    // Parse every key first (parse failures are fatal), grouping entries by the
    // chord they normalize to, in first-seen order.
    let mut order: Vec<KeyInput> = Vec::new();
    let mut groups: HashMap<KeyInput, Vec<(String, String)>> = HashMap::new();
    let mut tombstone_order: Vec<KeyInput> = Vec::new();
    let mut tombstone_set: std::collections::HashSet<KeyInput> = std::collections::HashSet::new();

    for (raw_key, raw_value) in entries {
        let chord = raw_key
            .parse::<KeyInput>()
            .map_err(|source| BuildError::KeyParse {
                key: raw_key.clone(),
                source,
            })?;

        match raw_value {
            RawValue::Bool(false) => {
                // Tombstone: record in unbinds (deduplicated, first-seen order).
                if tombstone_set.insert(chord) {
                    tombstone_order.push(chord);
                }
            }
            RawValue::Bool(true) => {
                return Err(BuildError::InvalidTombstone { key: raw_key });
            }
            RawValue::Action(action_name) => {
                let entry = groups.entry(chord).or_default();
                if entry.is_empty() {
                    order.push(chord);
                }
                entry.push((raw_key, action_name));
            }
        }
    }

    let mut keymap = Keymap::new();
    let mut names: HashMap<KeyInput, String> = HashMap::new();
    for chord in order {
        let Some(entries) = groups.remove(&chord) else {
            continue;
        };

        let mut resolved: Vec<(String, A)> = Vec::new();
        for (raw_key, action_name) in entries {
            match resolve(&action_name) {
                Some(action) => resolved.push((action_name, action)),
                None => warnings.push(Warning::UnknownAction {
                    key: raw_key,
                    action: action_name,
                }),
            }
        }

        if resolved.len() > 1 {
            let contenders: Vec<String> = resolved.iter().map(|(name, _)| name.clone()).collect();
            if let Some(winner) = contenders.last().cloned() {
                warnings.push(Warning::Conflict {
                    chord: chord.to_string(),
                    contenders,
                    winner,
                });
            }
        }

        // Last binding wins.
        if let Some((name, action)) = resolved.pop() {
            keymap.bind(chord, action);
            names.insert(chord, name);
        }
    }

    Ok((keymap, names, tombstone_order))
}

/// Serializes a [`Keymap`] and [`SequenceKeymap`] back into a TOML config string
/// — the inverse of [`from_str`].
///
/// `name_of` is the mirror of [`from_str`]'s `resolve`: it turns each bound
/// action `A` back into the name written in the config, returning `None` for an
/// action that has no config name. A binding whose action returns `None` is
/// **omitted** from the output (the write-side dual of how [`from_str`] reports
/// an unresolvable name as [`Warning::UnknownAction`]); pass a total `name_of`
/// for a faithful export.
///
/// The result is **deterministic**: the `[keys]` table is emitted in canonical
/// chord order and the `[[sequences]]` array in joined-chord order, so the same
/// maps always produce byte-identical output (suitable for writing to disk and
/// diffing). I/O is the caller's — like [`from_str`], this crate only deals in
/// `&str`/`String`.
///
/// The guarantee is a **semantic round-trip**, not byte identity:
/// `from_str(&to_toml(km, seq, name_of), resolve)` rebuilds keymaps equivalent to
/// `km`/`seq` when `name_of` and `resolve` are inverses. The reverse direction is
/// lossy by design — normalization (`shift+a` → `a`) is non-invertible, and
/// comments, ordering, and original spelling are not preserved.
///
/// Action names and chords are emitted by constructing a [`toml::Value`] (never
/// by string concatenation), so the `toml` serializer handles all escaping; a
/// name containing quotes or TOML metacharacters cannot break out of its string
/// to inject extra bindings on the round-trip.
///
/// # Trust boundary
///
/// `to_toml` applies **no** policy: it emits whatever is bound, reserved/escape
/// keys included. A caller that writes this output somewhere another process
/// (notably a PTY-internal process) can also write **must** enforce reserved-key
/// rejection at its own load/bind boundary — otherwise an attacker who can write
/// that file can rebind the escape key. See `docs/STATUS.md`.
///
/// # Panics
///
/// Never in practice: it serializes a TOML value built only from string keys and
/// values, which the `toml` serializer cannot fail on.
pub fn to_toml<A, F>(keymap: &Keymap<A>, sequences: &SequenceKeymap<A>, mut name_of: F) -> String
where
    F: FnMut(&A) -> Option<&str>,
{
    let mut root = toml::Table::new();

    let keys = keymap_to_table(keymap, &mut name_of);
    if !keys.is_empty() {
        root.insert("keys".to_string(), toml::Value::Table(keys));
    }
    insert_sequences(&mut root, sequences, &mut name_of);

    // Serializing a table of string-only values is infallible.
    toml::to_string(&root).expect("string-only TOML value always serializes")
}

/// Serializes a named-layer set and the global [`SequenceKeymap`] back into a
/// TOML config string — the inverse of [`from_str`] for a multi-layer config.
///
/// The [`GLOBAL_LAYER`] layer is emitted as the top-level `[keys]` table and every
/// other layer as `[layers.<name>]`, with `[[sequences]]` carrying the (global)
/// sequence map. Like [`to_toml`] the output is **deterministic** (`BTreeMap`/sorted
/// layer, chord, and sequence order) and applies **no** policy — the same trust-boundary
/// caveat applies (see [`to_toml`]). Pass [`BuildOutput::layers`] straight in to
/// round-trip a parsed config; an empty layer emits nothing, and a global-only set
/// produces byte-identical output to [`to_toml`] on that one layer.
///
/// To also emit tombstone entries (`"chord" = false`) for unbind declarations, use
/// [`to_toml_layered_with_unbinds`] instead.
///
/// # Panics
///
/// Never in practice, for the same reason as [`to_toml`].
pub fn to_toml_layered<A, F>(
    layers: &BTreeMap<String, Keymap<A>>,
    sequences: &SequenceKeymap<A>,
    mut name_of: F,
) -> String
where
    F: FnMut(&A) -> Option<&str>,
{
    to_toml_layered_impl(layers, sequences, &BTreeMap::new(), &mut name_of)
}

/// Serializes a named-layer set, the global [`SequenceKeymap`], and any unbind
/// declarations (`"chord" = false`) back into a TOML config string.
///
/// This is the tombstone-aware additive version of [`to_toml_layered`]: when
/// `unbinds` is non-empty, each chord in `unbinds[layer]` is emitted as
/// `"chord" = false` in the appropriate layer table. When `unbinds` is empty the
/// output is **byte-identical** to [`to_toml_layered`] — callers that never use
/// tombstones can use either function interchangeably.
///
/// Like all emit functions the output is deterministic and injection-safe: all
/// values are constructed as [`toml::Value`] (never by string concatenation), so
/// a chord whose display form contains TOML metacharacters cannot break out of its
/// string.
///
/// # Panics
///
/// Never in practice, for the same reason as [`to_toml`].
pub fn to_toml_layered_with_unbinds<A, F>(
    layers: &BTreeMap<String, Keymap<A>>,
    sequences: &SequenceKeymap<A>,
    unbinds: &BTreeMap<String, Vec<KeyInput>>,
    mut name_of: F,
) -> String
where
    F: FnMut(&A) -> Option<&str>,
{
    to_toml_layered_impl(layers, sequences, unbinds, &mut name_of)
}

/// Shared implementation for [`to_toml_layered`] and [`to_toml_layered_with_unbinds`].
fn to_toml_layered_impl<A, F>(
    layers: &BTreeMap<String, Keymap<A>>,
    sequences: &SequenceKeymap<A>,
    unbinds: &BTreeMap<String, Vec<KeyInput>>,
    name_of: &mut F,
) -> String
where
    F: FnMut(&A) -> Option<&str>,
{
    let mut root = toml::Table::new();
    let mut named = toml::Table::new();

    // Collect all layer names from both `layers` and `unbinds` so that a layer
    // with only tombstones (no remaining bindings) is still emitted.
    let mut all_layer_names: std::collections::BTreeSet<&str> =
        layers.keys().map(String::as_str).collect();
    for name in unbinds.keys() {
        all_layer_names.insert(name.as_str());
    }

    for name in all_layer_names {
        let mut table = if let Some(keymap) = layers.get(name) {
            keymap_to_table(keymap, name_of)
        } else {
            toml::Table::new()
        };

        // Append tombstone entries (`= false`) for this layer, using toml::Value
        // to avoid any string injection.
        if let Some(layer_unbinds) = unbinds.get(name) {
            for chord in layer_unbinds {
                table.insert(chord.to_string(), toml::Value::Boolean(false));
            }
        }

        if table.is_empty() {
            continue;
        }
        if name == GLOBAL_LAYER {
            root.insert("keys".to_string(), toml::Value::Table(table));
        } else {
            named.insert(name.to_string(), toml::Value::Table(table));
        }
    }

    insert_sequences(&mut root, sequences, name_of);
    if !named.is_empty() {
        root.insert("layers".to_string(), toml::Value::Table(named));
    }

    // Serializing a table of string-only values is infallible.
    toml::to_string(&root).expect("string-only TOML value always serializes")
}

/// The result of a [`merge`] operation: the merged output plus advisory notes.
///
/// Notes are kept **separate from [`BuildOutput::warnings`]** so that callers
/// gating on `output.warnings.is_empty()` (e.g. `deny_warnings`) are not
/// triggered by legitimate override operations. A note is information the caller
/// may want to surface or log; it is never an error.
#[derive(Debug)]
#[non_exhaustive]
pub struct Merged<A> {
    /// The merged build output. Its `warnings` field carries the union of
    /// `base.warnings` and `overlay.warnings` from the inputs; notes about the
    /// merge operation itself live in [`notes`](Self::notes).
    pub output: BuildOutput<A>,
    /// Advisory notes about the merge: overrides, unbinds, and dropped sequences.
    /// Never mixed into [`output.warnings`](BuildOutput::warnings).
    pub notes: Vec<MergeNote>,
}

/// An advisory note emitted by [`merge`].
///
/// All variants are display/audit information only; none indicate an error or
/// require action from the caller.
///
/// This is `#[non_exhaustive]` — future additive merge operations may add
/// further note kinds without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeNote {
    /// The overlay bound `chord` in `layer`, overriding an existing binding in
    /// the base. `chord` is in canonical form (e.g. `"ctrl+s"`).
    Overrode {
        /// The layer name in which the override occurred.
        layer: String,
        /// The chord that was overridden, in canonical form.
        chord: String,
    },
    /// The overlay declared `chord` in `layer` as a tombstone (`= false`), and
    /// the chord was removed from the base layer. `chord` is in canonical form.
    Unbound {
        /// The layer name from which the chord was removed.
        layer: String,
        /// The removed chord, in canonical form.
        chord: String,
    },
    /// The overlay's `unbinds` contained a tombstone for `chord` in `layer`,
    /// but the base had no binding for that chord — the tombstone was a no-op.
    UnbindMiss {
        /// The layer name.
        layer: String,
        /// The chord that was not found in the base, in canonical form.
        chord: String,
    },
    /// A sequence from the base was dropped because it shared a prefix with a
    /// sequence in the overlay (the overlay's sequence takes priority). The
    /// dropped sequence's chords are one canonical string per key.
    DroppedSequence {
        /// The chords of the dropped base sequence, one canonical string per key.
        sequence: Vec<String>,
    },
}

/// Merges a `base` (defaults) [`BuildOutput`] with an `overlay` (user config)
/// [`BuildOutput`], returning the combined result with advisory notes.
///
/// ## Merge semantics
///
/// - **Layers**: the layer sets are unioned. For each layer name present in
///   both `base` and `overlay`, bindings are merged chord-by-chord: the overlay
///   chord wins silently (it is the user's override). A [`MergeNote::Overrode`]
///   is emitted for each overridden chord.
/// - **Tombstones** (`overlay.unbinds`): for each chord listed as an unbind in
///   the overlay, the chord is removed from the corresponding base layer (if
///   present). A [`MergeNote::Unbound`] note is emitted for successful removals;
///   [`MergeNote::UnbindMiss`] when the chord was absent from the base.
/// - **Sequences**: the overlay's sequences are folded into the base's. An
///   exact-match sequence in the overlay silently replaces the base's. A sequence
///   in the overlay that is a prefix of (or is prefixed by) a sequence in the
///   base causes the base sequence to be dropped; a [`MergeNote::DroppedSequence`]
///   note is emitted.
/// - **Warnings**: the merged `output.warnings` is the concatenation of
///   `base.warnings` and `overlay.warnings`. Merge notes go to `notes`, never
///   to `output.warnings`.
///
/// Override operations are intentionally silent in `output.warnings` so that a
/// caller gating on `output.warnings.is_empty()` (deny-warnings mode) is not
/// triggered by the user legitimately overriding a default binding.
///
/// ## Bound: `A: Clone`
///
/// `merge` requires `A: Clone` because it copies actions from the overlay
/// into the merged map. [`SequenceKeymap::bindings`] yields shared references
/// only (there is no by-value iterator), so cloning is the only way to move
/// sequence actions across maps without unsafe code. This bound is on `merge`
/// alone and is not propagated to any core type.
#[must_use]
pub fn merge<A: Clone>(mut base: BuildOutput<A>, overlay: BuildOutput<A>) -> Merged<A> {
    let mut notes: Vec<MergeNote> = Vec::new();

    // Merge warnings: base first, then overlay.
    base.warnings.extend(overlay.warnings);

    // ── Tombstones ────────────────────────────────────────────────────────────
    // Apply overlay.unbinds to base.layers before merging bindings, so an
    // overlay that both unbinds and re-binds a chord gets a clean slate.
    for (layer_name, chords) in &overlay.unbinds {
        for chord in chords {
            let removed = base
                .layers
                .get_mut(layer_name)
                .and_then(|layer| layer.unbind(chord));
            if removed.is_some() {
                notes.push(MergeNote::Unbound {
                    layer: layer_name.clone(),
                    chord: chord.to_string(),
                });
            } else {
                notes.push(MergeNote::UnbindMiss {
                    layer: layer_name.clone(),
                    chord: chord.to_string(),
                });
            }
        }
    }

    // ── Layer bindings ────────────────────────────────────────────────────────
    // Move actions from the overlay layers into the base. `Keymap` exposes no
    // by-value iterator, so we use `iter()` to collect chord keys, then `unbind`
    // to take ownership of each action (moving, not cloning, for layer data).
    for (layer_name, overlay_keymap) in overlay.layers {
        let base_layer = base.layers.entry(layer_name.clone()).or_default();

        // Collect chords first (immutable borrow), then drain by value.
        let keys: Vec<KeyInput> = overlay_keymap.iter().map(|(k, _)| *k).collect();
        let mut owned_overlay = overlay_keymap;
        for chord in keys {
            if base_layer.contains(&chord) {
                notes.push(MergeNote::Overrode {
                    layer: layer_name.clone(),
                    chord: chord.to_string(),
                });
            }
            if let Some(action) = owned_overlay.unbind(&chord) {
                base_layer.bind(chord, action);
            }
        }
    }

    // ── Sequences ─────────────────────────────────────────────────────────────
    // Fold overlay sequences into the base. `SequenceKeymap::bindings` yields
    // `(Vec<KeyInput>, &A)` — shared references only; `A: Clone` is required
    // to copy actions across maps (documented in the function's bound).
    //
    // Strategy: collect all overlay (path, action) pairs first (cloning), then
    // bind each into the base. `SequenceKeymap::bind` returns `Err(PrefixShadow)`
    // when the new entry would conflict with an existing one in the *base*. In
    // that case, drop the conflicting base sequence and retry.
    let overlay_seqs: Vec<(Vec<KeyInput>, A)> = overlay
        .sequences
        .bindings()
        .map(|(path, action)| (path.clone(), action.clone()))
        .collect();

    for (path, action) in overlay_seqs {
        use keymap_seq::SeqBindError;
        // Retry loop: on each `PrefixShadow` we drop the conflicting base
        // sequence and retry until the bind succeeds (or a non-shadow error).
        while let Err(SeqBindError::PrefixShadow { sequence, conflict }) =
            base.sequences.bind(path.iter().copied(), action.clone())
        {
            // The conflicting path in the base must be dropped.
            let victim = if sequence == path { conflict } else { sequence };
            notes.push(MergeNote::DroppedSequence {
                sequence: render_sequence(&victim),
            });
            // Remove the victim from the base so the retry can succeed.
            // `SequenceKeymap` has no `unbind`; rebuild by collecting
            // all remaining bindings and starting fresh.
            let remaining: Vec<(Vec<KeyInput>, A)> = base
                .sequences
                .bindings()
                .filter(|(p, _)| *p != victim.as_slice())
                .map(|(p, a)| (p.clone(), a.clone()))
                .collect();
            base.sequences = SequenceKeymap::new();
            for (p, a) in remaining {
                // These were already valid (prefix-free among themselves),
                // so bind should not fail. Silently drop on error (should
                // not occur).
                let _ = base.sequences.bind(p.iter().copied(), a);
            }
        }
    }

    Merged {
        output: base,
        notes,
    }
}

/// Renders one keymap as a `chord -> action name` [`toml::Table`]. A `toml` table
/// is a sorted map, so chord order is canonical with no extra sorting; bindings
/// whose action has no config name (`name_of` returns `None`) are omitted.
fn keymap_to_table<A, F>(keymap: &Keymap<A>, name_of: &mut F) -> toml::Table
where
    F: FnMut(&A) -> Option<&str>,
{
    let mut table = toml::Table::new();
    for (chord, action) in keymap.iter() {
        if let Some(name) = name_of(action) {
            table.insert(chord.to_string(), toml::Value::String(name.to_string()));
        }
    }
    table
}

/// Inserts a `[[sequences]]` array into `root` when the map has any named
/// sequences, sorted by the joined canonical chords (the same ordering key
/// cross-shadow detection uses) for deterministic output.
fn insert_sequences<A, F>(root: &mut toml::Table, sequences: &SequenceKeymap<A>, name_of: &mut F)
where
    F: FnMut(&A) -> Option<&str>,
{
    let mut seqs: Vec<(Vec<String>, String)> = sequences
        .bindings()
        .filter_map(|(path, action)| {
            name_of(action).map(|name| (render_sequence(&path), name.to_string()))
        })
        .collect();
    seqs.sort_by_key(|(chords, _)| chords.join(" "));

    if seqs.is_empty() {
        return;
    }
    let array = seqs
        .into_iter()
        .map(|(chords, name)| {
            let mut entry = toml::Table::new();
            entry.insert(
                "keys".to_string(),
                toml::Value::Array(chords.into_iter().map(toml::Value::String).collect()),
            );
            entry.insert("action".to_string(), toml::Value::String(name));
            toml::Value::Table(entry)
        })
        .collect();
    root.insert("sequences".to_string(), toml::Value::Array(array));
}

/// Flags single-chord bindings that collide with the first key of a sequence.
///
/// The two maps are built independently and composed by the caller, so a chord
/// `j` and a sequence starting with `j` are not a build-time conflict in either
/// table alone — but together they make the press of `j` ambiguous. One advisory
/// [`Warning::SequenceShadow`] is emitted per offending chord (naming the
/// lexicographically-first sequence it shadows), chords visited in canonical-string
/// order for a deterministic warning sequence.
fn detect_cross_shadows(
    single_names: &HashMap<KeyInput, String>,
    seq_names: &HashMap<Vec<KeyInput>, String>,
    warnings: &mut Vec<Warning>,
) {
    let mut singles: Vec<(&KeyInput, &String)> = single_names.iter().collect();
    singles.sort_by_key(|(chord, _)| chord.to_string());

    for (chord, chord_action) in singles {
        let mut shadowed: Vec<(&Vec<KeyInput>, &String)> = seq_names
            .iter()
            .filter(|(seq, _)| seq.first() == Some(chord))
            .collect();
        shadowed.sort_by_key(|(seq, _)| render_sequence(seq).join(" "));

        if let Some((sequence, sequence_action)) = shadowed.first() {
            warnings.push(Warning::SequenceShadow {
                chord: chord.to_string(),
                chord_action: chord_action.clone(),
                sequence: render_sequence(sequence),
                sequence_action: (*sequence_action).clone(),
            });
        }
    }
}

/// Builds the [`SequenceKeymap`] from the `[[sequences]]` tables, appending any
/// survivable problems to `warnings`.
///
/// Detection of prefix conflicts lives entirely in `keymap-seq`: this function
/// just calls [`SequenceKeymap::bind`] in file order and translates its result
/// into warnings, keeping only a sequence→action-name dictionary (names are this
/// crate's vocabulary, not `keymap-seq`'s) so it can name both sides of a clash.
///
/// Two write policies show through here, intentionally asymmetric:
/// - An exact re-binding of the same sequence is **last-wins** (overwrites) and
///   reported as a [`Warning::Conflict`], matching the single-chord `[keys]`
///   table.
/// - A *prefix* clash keeps the **earlier** binding and drops the later one
///   (reported as [`Warning::PrefixShadow`]); the established prefix path wins.
///
/// Returns the built map together with the sequence→action-name dictionary it
/// accrues, so the caller can detect cross-shadows against the single-key table
/// without re-deriving names.
fn build_sequences<A, F>(
    raw_sequences: Vec<RawSequence>,
    resolve: &mut F,
    warnings: &mut Vec<Warning>,
) -> Result<SequenceBuild<A>, BuildError>
where
    F: FnMut(&str) -> Option<A>,
{
    let mut sequences = SequenceKeymap::new();
    // Action *names* keyed by the sequence they bind, so a clash can name both
    // sides. This is naming data, not conflict detection — the trie owns that.
    let mut names: HashMap<Vec<KeyInput>, String> = HashMap::new();

    for raw_seq in raw_sequences {
        let mut keys = Vec::with_capacity(raw_seq.keys.len());
        for raw_key in &raw_seq.keys {
            let chord = raw_key
                .parse::<KeyInput>()
                .map_err(|source| BuildError::KeyParse {
                    key: raw_key.clone(),
                    source,
                })?;
            keys.push(chord);
        }

        let Some(action) = resolve(&raw_seq.action) else {
            warnings.push(Warning::UnknownAction {
                key: render_sequence(&keys).join(" "),
                action: raw_seq.action,
            });
            continue;
        };

        match sequences.bind(keys.iter().copied(), action) {
            Ok(None) => {
                names.insert(keys, raw_seq.action);
            }
            Ok(Some(_)) => {
                // Exact re-binding: last wins, reported like a repeated chord.
                let previous = names.insert(keys.clone(), raw_seq.action.clone());
                warnings.push(Warning::Conflict {
                    chord: render_sequence(&keys).join(" "),
                    contenders: vec![previous.unwrap_or_default(), raw_seq.action.clone()],
                    winner: raw_seq.action,
                });
            }
            Err(SeqBindError::Empty) => {
                warnings.push(Warning::EmptySequence {
                    action: raw_seq.action,
                });
            }
            Err(SeqBindError::PrefixShadow { sequence, conflict }) => {
                let conflict_action = names.get(&conflict).cloned().unwrap_or_default();
                // The shorter sequence is the prefix that wins; the longer is shadowed.
                let (prefix, prefix_action, shadowed, shadowed_action) =
                    if sequence.len() <= conflict.len() {
                        (sequence, raw_seq.action, conflict, conflict_action)
                    } else {
                        (conflict, conflict_action, sequence, raw_seq.action)
                    };
                warnings.push(Warning::PrefixShadow {
                    prefix: render_sequence(&prefix),
                    prefix_action,
                    shadowed: render_sequence(&shadowed),
                    shadowed_action,
                });
            }
            // `SeqBindError` is non-exhaustive: a future rejection reason we do
            // not yet model drops the binding rather than aborting the build.
            Err(_) => {}
        }
    }

    Ok((sequences, names))
}

/// Renders a sequence as one canonical chord string per key.
fn render_sequence(keys: &[KeyInput]) -> Vec<String> {
    keys.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keymap_core::{Key, Modifiers};

    #[derive(Debug, Clone, PartialEq)]
    enum Action {
        Quit,
        Save,
        Split,
        Top,
    }

    fn resolver(name: &str) -> Option<Action> {
        match name {
            "quit" => Some(Action::Quit),
            "save" => Some(Action::Save),
            "split" => Some(Action::Split),
            "top" => Some(Action::Top),
            _ => None,
        }
    }

    use keymap_seq::Match;

    fn seq(keys: &[(char, Modifiers)]) -> Vec<KeyInput> {
        keys.iter()
            .map(|&(c, m)| KeyInput::new(Key::Char(c), m))
            .collect()
    }

    #[test]
    fn builds_bindings_and_resolves_actions() {
        let toml = "[keys]\n\"ctrl+q\" = \"quit\"\n\"ctrl+s\" = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Quit));
    }

    #[test]
    fn bare_keys_build_the_global_layer_which_is_always_present() {
        // Even an empty config has a (empty) global layer, so `global()` never
        // panics and callers can rely on `layers["global"]` existing.
        let empty: BuildOutput<Action> = from_str("", resolver).unwrap();
        assert_eq!(empty.layers.keys().collect::<Vec<_>>(), vec![GLOBAL_LAYER]);
        assert!(empty.global().is_empty());

        let out = from_str("[keys]\n\"ctrl+q\" = \"quit\"\n", resolver).unwrap();
        assert_eq!(out.layers.keys().collect::<Vec<_>>(), vec![GLOBAL_LAYER]);
    }

    #[test]
    fn named_layers_are_parsed_under_their_names() {
        let toml = "\
[keys]\n\"ctrl+q\" = \"quit\"\n\
[layers.panel]\n\"ctrl+s\" = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
        // global and panel both present, named exactly.
        assert_eq!(
            out.layers.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["global", "panel"]
        );
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        let s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Quit));
        assert_eq!(out.layers["panel"].get(&s), Some(&Action::Split));
        // A chord lives only in the layer that bound it.
        assert_eq!(out.global().get(&s), None);
        assert_eq!(out.layers["panel"].get(&q), None);
    }

    #[test]
    fn a_layer_with_no_keys_section_still_gets_an_empty_global() {
        let toml = "[layers.panel]\n\"ctrl+s\" = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.global().is_empty());
        assert!(!out.layers["panel"].is_empty());
    }

    #[test]
    fn same_chord_in_two_layers_is_an_override_not_a_conflict() {
        // The crux of the layered design: `ctrl+s` bound in both global and panel
        // is exactly the override `resolve_layered` exists for. The library never
        // knows which layer is active, so it must not report across the boundary.
        let toml = "\
[keys]\n\"ctrl+s\" = \"save\"\n\
[layers.panel]\n\"ctrl+s\" = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(
            out.warnings.is_empty(),
            "cross-layer override must not warn: {:?}",
            out.warnings
        );
        let s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(out.global().get(&s), Some(&Action::Save));
        assert_eq!(out.layers["panel"].get(&s), Some(&Action::Split));
    }

    #[test]
    fn explicit_global_layer_merges_into_the_top_level_keys() {
        let toml = "\
[keys]\n\"ctrl+q\" = \"quit\"\n\
[layers.global]\n\"ctrl+s\" = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
        // No separate "global" layer is created beyond the merged one.
        assert_eq!(out.layers.keys().collect::<Vec<_>>(), vec![GLOBAL_LAYER]);
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        let s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Quit));
        assert_eq!(out.global().get(&s), Some(&Action::Save));
    }

    #[test]
    fn keys_and_explicit_global_colliding_on_a_chord_conflict_last_wins() {
        // The same chord in `[keys]` and `[layers.global]` is a within-layer
        // conflict (they are the same layer): reported, and the `[layers.global]`
        // entry — fed after `[keys]` — wins.
        let toml = "\
[keys]\n\"ctrl+q\" = \"quit\"\n\
[layers.global]\n\"ctrl+q\" = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::Conflict {
                chord: "ctrl+q".to_string(),
                contenders: vec!["quit".to_string(), "save".to_string()],
                winner: "save".to_string(),
            }]
        );
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Save));
    }

    #[test]
    fn conflict_within_a_named_layer_is_reported() {
        // `ctrl+a` and `control+a` are the same chord spelled two ways; within one
        // layer that is a conflict, exactly as it is in the global layer.
        let toml = "\
[layers.panel]\n\"ctrl+a\" = \"quit\"\n\"control+a\" = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::Conflict {
                chord: "ctrl+a".to_string(),
                contenders: vec!["save".to_string(), "quit".to_string()],
                winner: "quit".to_string(),
            }]
        );
    }

    #[test]
    fn cross_shadow_is_checked_against_global_only() {
        // A chord `j` lives in `panel`, while the (global) sequence `j k` lives in
        // the global config. Because sequences are global and cross-shadow is a
        // global-layer-only check, the panel chord is *not* flagged.
        let toml = "\
[layers.panel]\n\"j\" = \"top\"\n\
[[sequences]]\nkeys = [\"j\", \"k\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(
            out.warnings.is_empty(),
            "a non-global chord must not cross-shadow a global sequence: {:?}",
            out.warnings
        );
    }

    #[test]
    fn unknown_action_in_a_named_layer_is_a_warning_carrying_no_layer_name() {
        // A named layer goes through the same pipeline as global, so an unknown
        // action there is a `UnknownAction` warning (binding skipped, not fatal).
        // By design the warning names the chord, not the layer.
        let toml = "[layers.panel]\n\"ctrl+z\" = \"undo\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::UnknownAction {
                key: "ctrl+z".to_string(),
                action: "undo".to_string(),
            }]
        );
        assert!(out.layers["panel"].is_empty());
    }

    #[test]
    fn malformed_key_in_a_named_layer_is_a_fatal_error() {
        // Parse failures are fatal in any layer, not just `[keys]`.
        let toml = "[layers.panel]\n\"ctrl+nope\" = \"quit\"\n";
        let err = from_str(toml, resolver).unwrap_err();
        assert!(matches!(err, BuildError::KeyParse { .. }));
    }

    #[test]
    fn sequences_do_not_create_extra_layers() {
        // A global-only config with sequences must still yield exactly the global
        // layer — sequences live beside the layers, not as one.
        let toml = "\
[keys]\n\"ctrl+q\" = \"quit\"\n\
[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
        assert_eq!(out.layers.keys().collect::<Vec<_>>(), vec![GLOBAL_LAYER]);
        assert!(!out.sequences.is_empty());
    }

    #[test]
    fn unknown_actions_across_layers_warn_global_first_then_name_order() {
        // Warning order is deterministic: global first, then named layers in
        // `BTreeMap` (lexicographic) name order.
        let toml = "\
[keys]\n\"a\" = \"nope_global\"\n\
[layers.zeta]\n\"b\" = \"nope_zeta\"\n\
[layers.alpha]\n\"c\" = \"nope_alpha\"\n";
        let out = from_str(toml, resolver).unwrap();
        let unknown_actions: Vec<&str> = out
            .warnings
            .iter()
            .filter_map(|w| match w {
                Warning::UnknownAction { action, .. } => Some(action.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            unknown_actions,
            vec!["nope_global", "nope_alpha", "nope_zeta"]
        );
    }

    #[test]
    fn unknown_top_level_field_is_a_fatal_error() {
        // `deny_unknown_fields`: a misspelled table is rejected, not ignored.
        let err = from_str("[kesy]\n\"ctrl+q\" = \"quit\"\n", resolver).unwrap_err();
        assert!(matches!(err, BuildError::Toml(_)));
    }

    #[test]
    fn unknown_sequence_field_is_a_fatal_error() {
        // Guards the future `layer = "..."` extension: an unsupported sequence
        // field fails loudly today rather than being silently dropped.
        let toml = "[[sequences]]\nkeys = [\"g\"]\naction = \"top\"\nlayer = \"panel\"\n";
        let err = from_str(toml, resolver).unwrap_err();
        assert!(matches!(err, BuildError::Toml(_)));
    }

    #[test]
    fn unknown_action_is_a_warning_not_a_failure() {
        let toml = "[keys]\n\"ctrl+q\" = \"quit\"\n\"ctrl+z\" = \"undo\"\n";
        let out = from_str(toml, resolver).unwrap();
        // The good binding still works.
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Quit));
        assert_eq!(
            out.warnings,
            vec![Warning::UnknownAction {
                key: "ctrl+z".to_string(),
                action: "undo".to_string(),
            }]
        );
    }

    #[test]
    fn distinct_spellings_of_same_chord_conflict() {
        // Different spellings of the same chord: "ctrl+a" and the alias
        // "control+a". (Note "ctrl+A" would be a *different* chord, since the
        // glyph case is significant by design.)
        let toml = "[keys]\n\"ctrl+a\" = \"quit\"\n\"control+a\" = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        let a = KeyInput::new(Key::Char('a'), Modifiers::CTRL);
        // One winner is bound for the shared chord.
        assert!(out.global().get(&a).is_some());
        let conflicts: Vec<_> = out
            .warnings
            .iter()
            .filter(|w| matches!(w, Warning::Conflict { .. }))
            .collect();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn legacy_lints_are_opt_in_and_separate_from_warnings() {
        // `cmd+s` is a perfectly valid binding — the config is clean — but it
        // won't reach a legacy terminal. That is NOT a build warning; it surfaces
        // only when the caller opts in by calling `legacy_lints`.
        let toml = "[keys]\n\"cmd+s\" = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
        assert_eq!(
            keymap_core::legacy_lints(out.global()),
            vec![keymap_core::LegacyLint::Unrepresentable {
                chord: "super+s".to_string(),
            }]
        );
    }

    #[test]
    fn malformed_key_is_a_fatal_error() {
        let toml = "[keys]\n\"ctrl+nope\" = \"quit\"\n";
        let err = from_str(toml, resolver).unwrap_err();
        assert!(matches!(err, BuildError::KeyParse { .. }));
    }

    #[test]
    fn malformed_toml_is_a_fatal_error() {
        let err = from_str("this is not toml", resolver).unwrap_err();
        assert!(matches!(err, BuildError::Toml(_)));
    }

    #[test]
    fn empty_config_builds_empty_map() {
        let out: BuildOutput<Action> = from_str("", resolver).unwrap();
        assert!(out.global().is_empty());
        assert!(out.sequences.is_empty());
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn builds_sequence_bindings() {
        let toml = "\
[keys]\n\"ctrl+q\" = \"quit\"\n\
[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"save\"\n\
[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+c\"]\naction = \"quit\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
        // Single chord still lands in the single-key map.
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Quit));
        // The sequence resolves; a partial buffer is a prefix.
        let save = seq(&[('x', Modifiers::CTRL), ('s', Modifiers::CTRL)]);
        assert_eq!(out.sequences.lookup(&save), Match::Exact(&Action::Save));
        assert_eq!(out.sequences.lookup(&save[..1]), Match::Prefix);
    }

    #[test]
    fn sequence_unknown_action_is_a_warning() {
        let toml = "[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+z\"]\naction = \"undo\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.sequences.is_empty());
        assert_eq!(
            out.warnings,
            vec![Warning::UnknownAction {
                key: "ctrl+x ctrl+z".to_string(),
                action: "undo".to_string(),
            }]
        );
    }

    #[test]
    fn sequence_prefix_shadow_is_a_warning_and_drops_the_later() {
        // `g` (top) then `g g` (split): the shorter shadows the longer.
        let toml = "\
[[sequences]]\nkeys = [\"g\"]\naction = \"top\"\n\
[[sequences]]\nkeys = [\"g\", \"g\"]\naction = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        // The earlier (shorter) binding survives; the later is dropped.
        assert_eq!(
            out.sequences.lookup(&seq(&[('g', Modifiers::NONE)])),
            Match::Exact(&Action::Top)
        );
        assert_eq!(
            out.warnings,
            vec![Warning::PrefixShadow {
                prefix: vec!["g".to_string()],
                prefix_action: "top".to_string(),
                shadowed: vec!["g".to_string(), "g".to_string()],
                shadowed_action: "split".to_string(),
            }]
        );
    }

    #[test]
    fn sequence_prefix_shadow_reverse_order_drops_the_later_short_one() {
        // Longer bound first, then the shorter prefix arrives: the shorter is the
        // later binding and is dropped, but it is still labelled the `prefix`.
        let toml = "\
[[sequences]]\nkeys = [\"g\", \"g\"]\naction = \"split\"\n\
[[sequences]]\nkeys = [\"g\"]\naction = \"top\"\n";
        let out = from_str(toml, resolver).unwrap();
        // The earlier (longer) binding survives.
        assert_eq!(
            out.sequences
                .lookup(&seq(&[('g', Modifiers::NONE), ('g', Modifiers::NONE)])),
            Match::Exact(&Action::Split)
        );
        assert_eq!(
            out.warnings,
            vec![Warning::PrefixShadow {
                prefix: vec!["g".to_string()],
                prefix_action: "top".to_string(),
                shadowed: vec!["g".to_string(), "g".to_string()],
                shadowed_action: "split".to_string(),
            }]
        );
    }

    #[test]
    fn same_sequence_three_times_reports_pairwise_conflicts() {
        let toml = "\
[[sequences]]\nkeys = [\"ctrl+x\"]\naction = \"save\"\n\
[[sequences]]\nkeys = [\"ctrl+x\"]\naction = \"split\"\n\
[[sequences]]\nkeys = [\"ctrl+x\"]\naction = \"quit\"\n";
        let out = from_str(toml, resolver).unwrap();
        // Last one wins.
        assert_eq!(
            out.sequences.lookup(&seq(&[('x', Modifiers::CTRL)])),
            Match::Exact(&Action::Quit)
        );
        // Two pairwise conflicts, each naming the running winner vs the newcomer.
        assert_eq!(
            out.warnings,
            vec![
                Warning::Conflict {
                    chord: "ctrl+x".to_string(),
                    contenders: vec!["save".to_string(), "split".to_string()],
                    winner: "split".to_string(),
                },
                Warning::Conflict {
                    chord: "ctrl+x".to_string(),
                    contenders: vec!["split".to_string(), "quit".to_string()],
                    winner: "quit".to_string(),
                },
            ]
        );
    }

    #[test]
    fn empty_sequence_is_a_warning() {
        let toml = "[[sequences]]\nkeys = []\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.sequences.is_empty());
        assert_eq!(
            out.warnings,
            vec![Warning::EmptySequence {
                action: "save".to_string(),
            }]
        );
    }

    #[test]
    fn duplicate_sequence_conflicts_and_last_wins() {
        let toml = "\
[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"save\"\n\
[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        let s = seq(&[('x', Modifiers::CTRL), ('s', Modifiers::CTRL)]);
        assert_eq!(out.sequences.lookup(&s), Match::Exact(&Action::Split));
        assert_eq!(
            out.warnings,
            vec![Warning::Conflict {
                chord: "ctrl+x ctrl+s".to_string(),
                contenders: vec!["save".to_string(), "split".to_string()],
                winner: "split".to_string(),
            }]
        );
    }

    #[test]
    fn malformed_sequence_key_is_a_fatal_error() {
        let toml = "[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+nope\"]\naction = \"save\"\n";
        let err = from_str(toml, resolver).unwrap_err();
        assert!(matches!(err, BuildError::KeyParse { .. }));
    }

    #[test]
    fn single_chord_shadowing_a_sequence_is_an_advisory_warning() {
        // `j` (down) and a sequence `j j` (the vim `jj` case): pressing `j` is
        // ambiguous without a timeout. Both bindings are kept — only flagged.
        let toml = "\
[keys]\n\"j\" = \"top\"\n\
[[sequences]]\nkeys = [\"j\", \"k\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        // Nothing dropped: the chord resolves and the sequence resolves.
        assert_eq!(
            out.global()
                .get(&KeyInput::new(Key::Char('j'), Modifiers::NONE)),
            Some(&Action::Top)
        );
        assert_eq!(
            out.sequences
                .lookup(&seq(&[('j', Modifiers::NONE), ('k', Modifiers::NONE)])),
            Match::Exact(&Action::Save)
        );
        assert_eq!(
            out.warnings,
            vec![Warning::SequenceShadow {
                chord: "j".to_string(),
                chord_action: "top".to_string(),
                sequence: vec!["j".to_string(), "k".to_string()],
                sequence_action: "save".to_string(),
            }]
        );
    }

    #[test]
    fn length_one_sequence_equal_to_a_single_chord_shadows() {
        // A degenerate overlap: the single chord and a length-1 sequence are the
        // exact same key, bound in both maps.
        let toml = "\
[keys]\n\"q\" = \"quit\"\n\
[[sequences]]\nkeys = [\"q\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::SequenceShadow {
                chord: "q".to_string(),
                chord_action: "quit".to_string(),
                sequence: vec!["q".to_string()],
                sequence_action: "save".to_string(),
            }]
        );
    }

    #[test]
    fn disjoint_first_keys_do_not_shadow() {
        // `j` single, `g g` sequence: different first keys, no overlap.
        let toml = "\
[keys]\n\"j\" = \"top\"\n\
[[sequences]]\nkeys = [\"g\", \"g\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn one_chord_shadowing_several_sequences_reports_the_lex_first() {
        // `j` starts both `j k` and `j a`; one advisory names the lex-first (`j a`).
        let toml = "\
[keys]\n\"j\" = \"top\"\n\
[[sequences]]\nkeys = [\"j\", \"k\"]\naction = \"save\"\n\
[[sequences]]\nkeys = [\"j\", \"a\"]\naction = \"quit\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::SequenceShadow {
                chord: "j".to_string(),
                chord_action: "top".to_string(),
                sequence: vec!["j".to_string(), "a".to_string()],
                sequence_action: "quit".to_string(),
            }]
        );
    }

    #[test]
    fn multiple_shadowing_chords_emit_in_canonical_chord_order() {
        // `z` is declared before `j`, but warnings come out chord-sorted (`j`
        // before `z`) — pins the sort, which the HashMap source order would not
        // otherwise guarantee.
        let toml = "\
[keys]\n\"z\" = \"top\"\n\"j\" = \"quit\"\n\
[[sequences]]\nkeys = [\"z\", \"x\"]\naction = \"save\"\n\
[[sequences]]\nkeys = [\"j\", \"k\"]\naction = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![
                Warning::SequenceShadow {
                    chord: "j".to_string(),
                    chord_action: "quit".to_string(),
                    sequence: vec!["j".to_string(), "k".to_string()],
                    sequence_action: "split".to_string(),
                },
                Warning::SequenceShadow {
                    chord: "z".to_string(),
                    chord_action: "top".to_string(),
                    sequence: vec!["z".to_string(), "x".to_string()],
                    sequence_action: "save".to_string(),
                },
            ]
        );
    }

    #[test]
    fn unknown_single_chord_does_not_shadow() {
        // `j` resolves to nothing, so it never enters the keymap — it cannot
        // shadow. Only the UnknownAction warning fires, no SequenceShadow.
        let toml = "\
[keys]\n\"j\" = \"nonexistent\"\n\
[[sequences]]\nkeys = [\"j\", \"k\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::UnknownAction {
                key: "j".to_string(),
                action: "nonexistent".to_string(),
            }]
        );
    }

    #[test]
    fn cross_shadow_coexists_with_conflict_and_comes_last() {
        // A single-key Conflict and a SequenceShadow in one config: detection
        // appends after the key/sequence passes, so the shadow is emitted last.
        let toml = "\
[keys]\n\"ctrl+a\" = \"quit\"\n\"control+a\" = \"save\"\n\"j\" = \"top\"\n\
[[sequences]]\nkeys = [\"j\", \"k\"]\naction = \"split\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![
                // The `[keys]` table is a BTreeMap, so "control+a" (save) sorts
                // before "ctrl+a" (quit); save is the earlier contender, quit wins.
                Warning::Conflict {
                    chord: "ctrl+a".to_string(),
                    contenders: vec!["save".to_string(), "quit".to_string()],
                    winner: "quit".to_string(),
                },
                Warning::SequenceShadow {
                    chord: "j".to_string(),
                    chord_action: "top".to_string(),
                    sequence: vec!["j".to_string(), "k".to_string()],
                    sequence_action: "split".to_string(),
                },
            ]
        );
    }

    #[test]
    fn chord_matching_a_non_first_sequence_key_does_not_shadow() {
        // `j` is only the *second* key of `g j`; the predicate is `first() ==`,
        // not "contains", so this must not warn.
        let toml = "\
[keys]\n\"j\" = \"top\"\n\
[[sequences]]\nkeys = [\"g\", \"j\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
    }

    // --- to_toml round-trip ---
    //
    // These use `String` as the action type, so `name_of`/`resolve` are exact
    // inverses (`|a| Some(a)` / `|n| Some(n.to_owned())`) and any round-trip
    // failure is the serializer's, not the resolver's.

    fn km_pairs(km: &Keymap<String>) -> Vec<(KeyInput, String)> {
        let mut v: Vec<_> = km.iter().map(|(k, a)| (*k, a.clone())).collect();
        v.sort_by_key(|(k, _)| k.to_string());
        v
    }

    fn seq_pairs(s: &SequenceKeymap<String>) -> Vec<(Vec<KeyInput>, String)> {
        let mut v: Vec<_> = s.bindings().map(|(p, a)| (p, a.clone())).collect();
        v.sort_by_key(|(p, _)| render_sequence(p).join(" "));
        v
    }

    /// `to_toml` then `from_str` reproduces the same bindings.
    fn assert_round_trips(km: &Keymap<String>, seq: &SequenceKeymap<String>) {
        let toml = to_toml(km, seq, |a: &String| Some(a.as_str()));
        let out = from_str(&toml, |name: &str| Some(name.to_owned())).unwrap();
        assert!(
            out.warnings.is_empty(),
            "round-trip warned: {:?}",
            out.warnings
        );
        assert_eq!(km_pairs(km), km_pairs(out.global()));
        assert_eq!(seq_pairs(seq), seq_pairs(&out.sequences));
    }

    fn norm(key: Key, mods: Modifiers) -> KeyInput {
        KeyInput::normalized(key, mods)
    }

    #[test]
    fn to_toml_round_trips_keys_and_sequences() {
        let mut km = Keymap::new();
        km.bind(norm(Key::Char('q'), Modifiers::CTRL), "quit".to_owned());
        km.bind(norm(Key::Char('s'), Modifiers::CTRL), "save".to_owned());
        // Normalization fixed point: bare shift+letter folds to the letter, so it
        // emits as "a" and round-trips to the same normalized chord.
        km.bind(norm(Key::Char('a'), Modifiers::SHIFT), "all".to_owned());
        // Multi-modifier keeps shift: "ctrl+shift+a" survives as itself.
        km.bind(
            norm(Key::Char('a'), Modifiers::CTRL | Modifiers::SHIFT),
            "alt_all".to_owned(),
        );

        let mut seq = SequenceKeymap::new();
        seq.bind(
            [
                norm(Key::Char('x'), Modifiers::CTRL),
                norm(Key::Char('s'), Modifiers::CTRL),
            ],
            "seq_save".to_owned(),
        )
        .unwrap();
        seq.bind(
            [
                norm(Key::Char('g'), Modifiers::NONE),
                norm(Key::Char('g'), Modifiers::NONE),
            ],
            "top".to_owned(),
        )
        .unwrap();

        assert_round_trips(&km, &seq);
    }

    #[test]
    fn to_toml_is_deterministic_and_canonically_ordered() {
        let mut km = Keymap::new();
        km.bind(norm(Key::Char('z'), Modifiers::CTRL), "z".to_owned());
        km.bind(norm(Key::Char('a'), Modifiers::CTRL), "a".to_owned());
        let seq = SequenceKeymap::new();

        let first = to_toml(&km, &seq, |a: &String| Some(a.as_str()));
        let second = to_toml(&km, &seq, |a: &String| Some(a.as_str()));
        assert_eq!(first, second, "output must be deterministic");
        // Canonical chord order: "ctrl+a" before "ctrl+z" regardless of bind order.
        let a_at = first.find("ctrl+a").unwrap();
        let z_at = first.find("ctrl+z").unwrap();
        assert!(
            a_at < z_at,
            "keys must be emitted in canonical order:\n{first}"
        );
    }

    #[test]
    fn to_toml_omits_unnameable_bindings() {
        let mut km = Keymap::new();
        km.bind(norm(Key::Char('q'), Modifiers::CTRL), "quit".to_owned());
        km.bind(norm(Key::Char('x'), Modifiers::CTRL), "secret".to_owned());
        let seq = SequenceKeymap::new();

        // `name_of` declines to name "secret", so that binding is dropped.
        let toml = to_toml(&km, &seq, |a: &String| {
            (a != "secret").then_some(a.as_str())
        });
        let out = from_str(&toml, |name: &str| Some(name.to_owned())).unwrap();
        assert_eq!(
            out.global().get(&norm(Key::Char('q'), Modifiers::CTRL)),
            Some(&"quit".to_owned())
        );
        assert_eq!(
            out.global().get(&norm(Key::Char('x'), Modifiers::CTRL)),
            None
        );
    }

    #[test]
    fn to_toml_empty_maps_emit_empty_string() {
        let km: Keymap<String> = Keymap::new();
        let seq: SequenceKeymap<String> = SequenceKeymap::new();
        assert_eq!(to_toml(&km, &seq, |a: &String| Some(a.as_str())), "");
    }

    #[test]
    fn to_toml_round_trips_adversarial_names_and_chords() {
        // Action names with TOML metacharacters must not break out of their
        // string and inject bindings; emitting via `toml::Value` escapes them.
        let mut km = Keymap::new();
        km.bind(
            norm(Key::Char('a'), Modifiers::NONE),
            "quit\"; [injected]\nx = \"oops".to_owned(),
        );
        // Chords whose glyph collides with grammar/TOML punctuation.
        km.bind(norm(Key::Char('+'), Modifiers::NONE), "plus".to_owned());
        km.bind(norm(Key::Char(' '), Modifiers::NONE), "space".to_owned());
        km.bind(norm(Key::Char('"'), Modifiers::NONE), "quote".to_owned());
        km.bind(norm(Key::Char('あ'), Modifiers::NONE), "hira".to_owned());
        km.bind(norm(Key::Char('F'), Modifiers::NONE), "cap_f".to_owned());

        // Sequence starts with an unbound key (`z`) so it does not cross-shadow
        // the single-key ` `/`+` bindings; ` ` and `+` still appear as non-first
        // elements to exercise array-element round-tripping of odd glyphs.
        let mut seq = SequenceKeymap::new();
        seq.bind(
            [
                norm(Key::Char('z'), Modifiers::NONE),
                norm(Key::Char(' '), Modifiers::NONE),
                norm(Key::Char('+'), Modifiers::NONE),
            ],
            "z_space_plus".to_owned(),
        )
        .unwrap();

        assert_round_trips(&km, &seq);
    }

    #[test]
    fn shadow_matching_is_on_the_parsed_chord_not_the_label() {
        // Positive: `ctrl+x` single shadows `[ctrl+x, ctrl+s]`.
        let toml = "\
[keys]\n\"ctrl+x\" = \"top\"\n\
[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert_eq!(
            out.warnings,
            vec![Warning::SequenceShadow {
                chord: "ctrl+x".to_string(),
                chord_action: "top".to_string(),
                sequence: vec!["ctrl+x".to_string(), "ctrl+s".to_string()],
                sequence_action: "save".to_string(),
            }]
        );

        // Negative: a different modifier set is a different KeyInput, no shadow.
        let toml = "\
[keys]\n\"ctrl+x\" = \"top\"\n\
[[sequences]]\nkeys = [\"ctrl+shift+x\", \"ctrl+s\"]\naction = \"save\"\n";
        let out = from_str(toml, resolver).unwrap();
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn to_toml_layered_round_trips_named_layers() {
        let mut global = Keymap::new();
        global.bind(norm(Key::Char('q'), Modifiers::CTRL), "quit".to_owned());
        let mut panel = Keymap::new();
        panel.bind(norm(Key::Char('s'), Modifiers::CTRL), "split".to_owned());
        // The same chord in two layers is preserved independently on round-trip.
        panel.bind(
            norm(Key::Char('q'), Modifiers::CTRL),
            "panel_quit".to_owned(),
        );

        let mut layers = BTreeMap::new();
        layers.insert(GLOBAL_LAYER.to_string(), global);
        layers.insert("panel".to_string(), panel);

        let mut seq = SequenceKeymap::new();
        seq.bind(
            [
                norm(Key::Char('x'), Modifiers::CTRL),
                norm(Key::Char('s'), Modifiers::CTRL),
            ],
            "seq_save".to_owned(),
        )
        .unwrap();

        let toml = to_toml_layered(&layers, &seq, |a: &String| Some(a.as_str()));
        let out = from_str(&toml, |name: &str| Some(name.to_owned())).unwrap();
        assert!(
            out.warnings.is_empty(),
            "round-trip warned: {:?}",
            out.warnings
        );
        assert_eq!(km_pairs(&layers["global"]), km_pairs(out.global()));
        assert_eq!(km_pairs(&layers["panel"]), km_pairs(&out.layers["panel"]));
        assert_eq!(seq_pairs(&seq), seq_pairs(&out.sequences));
    }

    #[test]
    fn to_toml_layered_matches_to_toml_for_a_global_only_set() {
        // A set with only the global layer must emit byte-identical output to
        // `to_toml` on that one keymap, so the two emitters cannot drift.
        let mut global = Keymap::new();
        global.bind(norm(Key::Char('q'), Modifiers::CTRL), "quit".to_owned());
        global.bind(norm(Key::Char('s'), Modifiers::CTRL), "save".to_owned());
        let mut seq = SequenceKeymap::new();
        seq.bind(
            [
                norm(Key::Char('g'), Modifiers::NONE),
                norm(Key::Char('g'), Modifiers::NONE),
            ],
            "top".to_owned(),
        )
        .unwrap();

        let mut layers = BTreeMap::new();
        layers.insert(GLOBAL_LAYER.to_string(), global.clone());

        let plain = to_toml(&global, &seq, |a: &String| Some(a.as_str()));
        let layered = to_toml_layered(&layers, &seq, |a: &String| Some(a.as_str()));
        assert_eq!(plain, layered);
        // And an empty global-only set emits the empty string, like `to_toml`.
        let empty_layers: BTreeMap<String, Keymap<String>> = BTreeMap::new();
        assert_eq!(
            to_toml_layered(&empty_layers, &SequenceKeymap::new(), |a: &String| Some(
                a.as_str()
            )),
            ""
        );
    }

    #[test]
    fn to_toml_layered_drops_empty_layers() {
        // An empty layer emits no table, so it does not survive a round-trip.
        // This is the deliberate asymmetry: `from_str` keeps a declared-but-empty
        // layer in the map, but `to_toml_layered` writes only layers with bindings.
        let mut global = Keymap::new();
        global.bind(norm(Key::Char('q'), Modifiers::CTRL), "quit".to_owned());
        let mut layers = BTreeMap::new();
        layers.insert(GLOBAL_LAYER.to_string(), global);
        layers.insert("panel".to_string(), Keymap::<String>::new());

        let toml = to_toml_layered(&layers, &SequenceKeymap::new(), |a: &String| {
            Some(a.as_str())
        });
        assert!(
            !toml.contains("panel"),
            "empty layer must not be emitted:\n{toml}"
        );
        let out = from_str(&toml, |name: &str| Some(name.to_owned())).unwrap();
        assert_eq!(out.layers.keys().collect::<Vec<_>>(), vec![GLOBAL_LAYER]);
    }

    // ─── chord tombstone (= false) tests ───────────────────────────────────────

    /// In 0.1.0 `"chord" = false` produced a `BuildError::Toml`. This test
    /// pins that it is now `Allowed` (returns a `BuildOutput`, not an error)
    /// and the chord lands in `unbinds`, not in the keymap.
    #[test]
    fn tombstone_false_was_build_error_in_0_1_0_now_accepted() {
        // The key fact: this used to error; it now succeeds.
        let toml = "[keys]\n\"ctrl+q\" = false\n";
        let out = from_str(toml, resolver).unwrap();
        // The chord is NOT in the keymap.
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), None);
        // The chord IS in unbinds.
        let global_unbinds = out
            .unbinds
            .get(GLOBAL_LAYER)
            .expect("global unbinds present");
        assert!(global_unbinds.contains(&q));
        // No warnings: tombstones are silent.
        assert!(out.warnings.is_empty());
    }

    /// `"chord" = true` remains a `BuildError::InvalidTombstone`.
    #[test]
    fn tombstone_true_is_still_an_error() {
        let toml = "[keys]\n\"ctrl+q\" = true\n";
        let err = from_str(toml, resolver).unwrap_err();
        assert!(
            matches!(err, BuildError::InvalidTombstone { .. }),
            "expected InvalidTombstone, got {err:?}"
        );
    }

    #[test]
    fn tombstone_in_named_layer_lands_in_unbinds_not_keymap() {
        let toml = "[layers.panel]\n\"ctrl+s\" = false\n";
        let out = from_str(toml, resolver).unwrap();
        let s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        assert!(out.layers["panel"].is_empty());
        let panel_unbinds = out.unbinds.get("panel").expect("panel unbinds present");
        assert!(panel_unbinds.contains(&s));
    }

    #[test]
    fn tombstone_coexists_with_bindings_in_same_layer() {
        // A layer can have both a binding and a tombstone.
        let toml = "[keys]\n\"ctrl+q\" = \"quit\"\n\"ctrl+s\" = false\n";
        let out = from_str(toml, resolver).unwrap();
        let q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        let s = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(out.global().get(&q), Some(&Action::Quit));
        assert_eq!(out.global().get(&s), None);
        let global_unbinds = out.unbinds.get(GLOBAL_LAYER).unwrap();
        assert!(global_unbinds.contains(&s));
    }

    #[test]
    fn empty_config_has_empty_unbinds() {
        // Configs with no tombstones have an empty unbinds map.
        let out: BuildOutput<Action> =
            from_str("[keys]\n\"ctrl+q\" = \"quit\"\n", resolver).unwrap();
        assert!(out.unbinds.is_empty());
    }

    // ─── to_toml_layered tombstone round-trip tests ─────────────────────────

    /// When `unbinds` is empty, `to_toml_layered_with_unbinds` is byte-identical
    /// to `to_toml_layered` — this pins that the additive version does not change
    /// the output for callers that never use tombstones.
    #[test]
    fn to_toml_layered_with_empty_unbinds_matches_0_1_0_behavior() {
        let mut global = Keymap::new();
        global.bind(norm(Key::Char('q'), Modifiers::CTRL), "quit".to_owned());
        let mut layers = BTreeMap::new();
        layers.insert(GLOBAL_LAYER.to_string(), global.clone());
        let seq = SequenceKeymap::new();
        let empty_unbinds: BTreeMap<String, Vec<KeyInput>> = BTreeMap::new();

        let plain = to_toml_layered(&layers, &seq, |a: &String| Some(a.as_str()));
        let with_empty =
            to_toml_layered_with_unbinds(&layers, &seq, &empty_unbinds, |a: &String| {
                Some(a.as_str())
            });
        assert_eq!(
            plain, with_empty,
            "empty unbinds must produce byte-identical output to to_toml_layered"
        );
    }

    #[test]
    fn to_toml_layered_emits_tombstones_and_round_trips() {
        // A tombstone in the global layer round-trips through to_toml_layered →
        // from_str and appears again in unbinds.
        let global: Keymap<String> = Keymap::new(); // nothing bound
        let mut layers = BTreeMap::new();
        layers.insert(GLOBAL_LAYER.to_string(), global);

        let ctrl_s = norm(Key::Char('s'), Modifiers::CTRL);
        let mut unbinds: BTreeMap<String, Vec<KeyInput>> = BTreeMap::new();
        unbinds.insert(GLOBAL_LAYER.to_string(), vec![ctrl_s]);

        let toml_str = to_toml_layered_with_unbinds(
            &layers,
            &SequenceKeymap::new(),
            &unbinds,
            |a: &String| Some(a.as_str()),
        );
        // The tombstone must appear in the output.
        assert!(
            toml_str.contains("false"),
            "tombstone must be emitted as `= false`:\n{toml_str}"
        );
        // Round-trip: parsing the emitted TOML recovers the unbind.
        let out = from_str(&toml_str, |name: &str| Some(name.to_owned())).unwrap();
        let global_unbinds = out
            .unbinds
            .get(GLOBAL_LAYER)
            .expect("global unbinds present");
        assert!(global_unbinds.contains(&ctrl_s));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn to_toml_layered_tombstone_injection_safe() {
        // A chord whose string representation contains TOML metacharacters must
        // not be able to inject extra entries. The tombstone is emitted via
        // `toml::Value::Boolean(false)` — the same injection-safe path as strings.
        // This test uses a space chord (which emits as `" "`) to exercise quoting.
        let space = norm(Key::Char(' '), Modifiers::NONE);
        let layers: BTreeMap<String, Keymap<String>> = BTreeMap::new();
        let mut unbinds: BTreeMap<String, Vec<KeyInput>> = BTreeMap::new();
        unbinds.insert(GLOBAL_LAYER.to_string(), vec![space]);

        let toml_str = to_toml_layered_with_unbinds(
            &layers,
            &SequenceKeymap::new(),
            &unbinds,
            |a: &String| Some(a.as_str()),
        );
        // Must round-trip cleanly (no injection = no parse error).
        let out = from_str(&toml_str, |name: &str| Some(name.to_owned())).unwrap();
        let global_unbinds = out
            .unbinds
            .get(GLOBAL_LAYER)
            .expect("global unbinds present");
        assert!(global_unbinds.contains(&space));
    }

    // ─── merge tests ─────────────────────────────────────────────────────────

    fn make_output(toml: &str) -> BuildOutput<String> {
        from_str(toml, |name: &str| Some(name.to_owned())).unwrap()
    }

    #[test]
    fn merge_overlay_chord_wins_silently_with_override_note() {
        let base = make_output("[keys]\n\"ctrl+s\" = \"save_base\"\n");
        let overlay = make_output("[keys]\n\"ctrl+s\" = \"save_overlay\"\n");
        let merged = merge(base, overlay);

        let s = norm(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(
            merged.output.global().get(&s),
            Some(&"save_overlay".to_owned()),
            "overlay wins"
        );
        assert!(
            merged.output.warnings.is_empty(),
            "override must not produce a Warning"
        );
        assert!(
            merged.notes.iter().any(|n| matches!(n,
                MergeNote::Overrode { layer, chord }
                if layer == GLOBAL_LAYER && chord == "ctrl+s"
            )),
            "Overrode note must be emitted: {:?}",
            merged.notes
        );
    }

    #[test]
    fn merge_layer_union_adds_layers_from_overlay() {
        let base = make_output("[keys]\n\"ctrl+q\" = \"quit\"\n");
        let overlay = make_output("[layers.panel]\n\"ctrl+s\" = \"split\"\n");
        let merged = merge(base, overlay);

        assert!(
            merged.output.layers.contains_key("panel"),
            "panel layer added"
        );
        let s = norm(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(
            merged.output.layers["panel"].get(&s),
            Some(&"split".to_owned())
        );
        assert!(
            merged.notes.is_empty(),
            "no notes for pure add: {:?}",
            merged.notes
        );
    }

    #[test]
    fn merge_sequence_exact_overlay_wins() {
        let base =
            make_output("[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"save_base\"\n");
        let overlay = make_output(
            "[[sequences]]\nkeys = [\"ctrl+x\", \"ctrl+s\"]\naction = \"save_overlay\"\n",
        );
        let merged = merge(base, overlay);

        let xs = vec![
            norm(Key::Char('x'), Modifiers::CTRL),
            norm(Key::Char('s'), Modifiers::CTRL),
        ];
        assert_eq!(
            merged.output.sequences.lookup(&xs),
            keymap_seq::Match::Exact(&"save_overlay".to_owned()),
            "overlay sequence wins"
        );
        assert!(merged.output.warnings.is_empty());
    }

    #[test]
    fn merge_sequence_prefix_clash_drops_base_with_note() {
        // Base has `g g`; overlay has `g`. The overlay's shorter sequence is a
        // prefix of the base's longer one. The base sequence is dropped.
        let base = make_output("[[sequences]]\nkeys = [\"g\", \"g\"]\naction = \"top_base\"\n");
        let overlay = make_output("[[sequences]]\nkeys = [\"g\"]\naction = \"top_overlay\"\n");
        let merged = merge(base, overlay);

        let g = vec![norm(Key::Char('g'), Modifiers::NONE)];
        assert_eq!(
            merged.output.sequences.lookup(&g),
            keymap_seq::Match::Exact(&"top_overlay".to_owned()),
            "overlay wins"
        );
        assert!(
            merged
                .notes
                .iter()
                .any(|n| matches!(n, MergeNote::DroppedSequence { .. })),
            "DroppedSequence note must be emitted: {:?}",
            merged.notes
        );
    }

    #[test]
    fn merge_unbind_removes_from_base_with_unbound_note() {
        let base = make_output("[keys]\n\"ctrl+s\" = \"save\"\n");
        let overlay = make_output("[keys]\n\"ctrl+s\" = false\n");
        let merged = merge(base, overlay);

        let s = norm(Key::Char('s'), Modifiers::CTRL);
        assert_eq!(
            merged.output.global().get(&s),
            None,
            "chord removed from base"
        );
        assert!(
            merged.notes.iter().any(|n| matches!(n,
                MergeNote::Unbound { layer, chord }
                if layer == GLOBAL_LAYER && chord == "ctrl+s"
            )),
            "Unbound note must be emitted: {:?}",
            merged.notes
        );
        assert!(merged.output.warnings.is_empty());
    }

    #[test]
    fn merge_unbind_of_absent_chord_produces_miss_note() {
        let base = make_output("[keys]\n\"ctrl+q\" = \"quit\"\n");
        let overlay = make_output("[keys]\n\"ctrl+s\" = false\n");
        let merged = merge(base, overlay);

        assert!(
            merged.notes.iter().any(|n| matches!(n,
                MergeNote::UnbindMiss { chord, .. }
                if chord == "ctrl+s"
            )),
            "UnbindMiss note expected: {:?}",
            merged.notes
        );
        // The present binding is untouched.
        let q = norm(Key::Char('q'), Modifiers::CTRL);
        assert_eq!(merged.output.global().get(&q), Some(&"quit".to_owned()));
    }

    #[test]
    fn merge_warnings_are_concatenated_not_mixed_into_notes() {
        // Both base and overlay have warnings (unknown action names); they must
        // appear in output.warnings, not in notes. Use `resolver` which only
        // knows "quit"/"save"/"split"/"top" so "nope_*" is an UnknownAction.
        let base = from_str("[keys]\n\"ctrl+z\" = \"nope_base\"\n", resolver).unwrap();
        let overlay = from_str("[keys]\n\"ctrl+y\" = \"nope_overlay\"\n", resolver).unwrap();
        assert_eq!(base.warnings.len(), 1);
        assert_eq!(overlay.warnings.len(), 1);
        let merged = merge(base, overlay);

        assert_eq!(merged.output.warnings.len(), 2, "both warnings carried");
        assert!(merged.notes.is_empty());
    }

    // ─── WarningKind + Display for Warning (S5) ──────────────────────────────

    /// Every `Warning` variant maps to the corresponding `WarningKind` variant with
    /// no gaps — the table below pins the 1-to-1 correspondence.
    #[test]
    fn warning_kind_covers_all_variants() {
        let conflict = Warning::Conflict {
            chord: "ctrl+a".to_string(),
            contenders: vec!["quit".to_string(), "save".to_string()],
            winner: "save".to_string(),
        };
        assert_eq!(conflict.kind(), WarningKind::Conflict);

        let unknown = Warning::UnknownAction {
            key: "ctrl+z".to_string(),
            action: "undo".to_string(),
        };
        assert_eq!(unknown.kind(), WarningKind::UnknownAction);

        let prefix = Warning::PrefixShadow {
            prefix: vec!["g".to_string()],
            prefix_action: "top".to_string(),
            shadowed: vec!["g".to_string(), "g".to_string()],
            shadowed_action: "top2".to_string(),
        };
        assert_eq!(prefix.kind(), WarningKind::PrefixShadow);

        let empty = Warning::EmptySequence {
            action: "quit".to_string(),
        };
        assert_eq!(empty.kind(), WarningKind::EmptySequence);

        let shadow = Warning::SequenceShadow {
            chord: "j".to_string(),
            chord_action: "down".to_string(),
            sequence: vec!["j".to_string(), "k".to_string()],
            sequence_action: "top".to_string(),
        };
        assert_eq!(shadow.kind(), WarningKind::SequenceShadow);
    }

    /// Display output is non-empty, human-readable, and contains the key field
    /// for each variant. Does not assert exact format (format is not stable), but
    /// checks the load-bearing content the user would need to read.
    #[test]
    fn warning_display_is_human_readable() {
        let conflict = Warning::Conflict {
            chord: "ctrl+a".to_string(),
            contenders: vec!["quit".to_string(), "save".to_string()],
            winner: "save".to_string(),
        };
        let s = conflict.to_string();
        assert!(
            s.contains("ctrl+a"),
            "conflict display must mention the chord: {s}"
        );
        assert!(
            s.contains("save"),
            "conflict display must mention the winner: {s}"
        );

        let unknown = Warning::UnknownAction {
            key: "ctrl+z".to_string(),
            action: "undo".to_string(),
        };
        let s = unknown.to_string();
        assert!(
            s.contains("undo"),
            "unknown-action display must mention the action: {s}"
        );

        let prefix = Warning::PrefixShadow {
            prefix: vec!["g".to_string()],
            prefix_action: "top".to_string(),
            shadowed: vec!["g".to_string(), "g".to_string()],
            shadowed_action: "top2".to_string(),
        };
        let s = prefix.to_string();
        assert!(
            !s.is_empty(),
            "prefix-shadow display must not be empty: {s}"
        );

        let empty_seq = Warning::EmptySequence {
            action: "quit".to_string(),
        };
        let s = empty_seq.to_string();
        assert!(
            s.contains("quit"),
            "empty-sequence display must mention the action: {s}"
        );

        let shadow = Warning::SequenceShadow {
            chord: "j".to_string(),
            chord_action: "down".to_string(),
            sequence: vec!["j".to_string(), "k".to_string()],
            sequence_action: "top".to_string(),
        };
        let s = shadow.to_string();
        assert!(
            s.contains('j'),
            "sequence-shadow display must mention the chord: {s}"
        );
    }
}
