# Changelog

All notable changes to `keymap-config` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

## [0.1.1] - 2026-06-13

All changes are **additive**; no existing public API is removed or altered.

### Added

- `merge(base, overlay) -> Merged<A>` — composes a compiled-in defaults `Keymap`
  set with a user TOML overlay (the `BuildOutput` from `from_str`). Same-layer
  same-chord: overlay (user) wins silently — this is an intentional override, not
  a conflict, so it does **not** appear in `BuildOutput::warnings` and does not
  trip `deny_warnings()`. Override events go into `Merged::notes` (a `Vec<MergeNote>`)
  so the warnings gate is never tripped by a legitimate user override.
- `Merged<A>` (`#[non_exhaustive]`) — the output of `merge`: `output: BuildOutput<A>`,
  `notes: Vec<MergeNote>`. `A: Clone` bound (needed to copy actions from base layers
  into the merged output without requiring the caller to pass them twice).
- `MergeNote` (`#[non_exhaustive]`) — describes one override event from the merge:
  `Override { layer, chord, base_action_name, overlay_action_name }`.
- Chord tombstone: `"chord" = false` in `[keys]` / `[layers.<name>]` is now parsed
  as an **unbind declaration** (removes the chord from the active layer). **Forward
  compatibility note:** in 0.1.0 this was a `BuildError::Toml` (boolean values were
  unconditionally rejected). Starting 0.1.1 it is the new meaning of that
  previously-rejected input — additive because no existing valid config contained it.
  `= true` remains a `BuildError::Toml`. Existing configs that use only string values
  are completely unaffected.
- `BuildOutput::unbinds: Vec<KeyInput>` field — the chords that were explicitly
  unbound via tombstones in the TOML. Additive under `#[non_exhaustive]` on
  `BuildOutput`.
- `to_toml_layered_with_unbinds(layers, sequences, unbinds, name_of) -> String` —
  round-trip dual of the tombstone parser: emits `"chord" = false` for each
  unbound chord (omitted if empty). The signature of the existing `to_toml_layered`
  is **unchanged**; tombstone serialization is in this new function only.
- `WarningKind` — fieldless `#[non_exhaustive]` enum with variants matching the
  `Warning` variants: `Conflict`, `UnknownAction`, `PrefixShadow`, `EmptySequence`,
  `SequenceShadow`. Eliminates the ~20-line enum re-typing every caller currently
  needs when categorising `#[non_exhaustive]` `Warning` values without destructuring.
- `Warning::kind() -> WarningKind` — returns the `WarningKind` discriminant so
  callers can pattern-match on category (`match w.kind() { WarningKind::Conflict =>
  …, _ => {} }`) without exhausting all `Warning` variants.
- `impl Display for Warning` — one-line human-readable string, intended for logging
  and UI display; not specified as stable and should not be parsed.

## [0.1.0] - 2026-05-28

### Added

- `from_str(toml, resolve) -> Result<BuildOutput<A>, BuildError>` parses a TOML
  document into named `Keymap<A>` layers plus a `SequenceKeymap<A>`, resolving
  action names through the caller-supplied `resolve` closure (so the action
  type `A` never appears in this crate's public types).
- `BuildOutput { layers: BTreeMap<String, Keymap<A>>, sequences, warnings }`
  with `out.global()` as the accessor for the bare `[keys]` table (the always-
  present `GLOBAL_LAYER` named `"global"`); each `[layers.<name>]` table builds
  a layer of that name.
- Non-fatal `Warning` variants surfaced instead of build failures: `Conflict`,
  `UnknownAction`, `PrefixShadow`, `EmptySequence`, `SequenceShadow`.
  `#[non_exhaustive]`, so callers must `_` -arm.
- `to_toml(&keymap, &sequences, name_of) -> String` and
  `to_toml_layered(&layers, &sequences, name_of)` round-trip back to TOML
  (semantic round-trip, not byte identity).

[Unreleased]: https://github.com/S-Nakamur-a/keymap-rs/commits/main
[0.1.1]: https://crates.io/crates/keymap-config/0.1.1
[0.1.0]: https://crates.io/crates/keymap-config/0.1.0
