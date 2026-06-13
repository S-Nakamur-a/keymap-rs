# Changelog

All notable changes to `keymap-core` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

## [0.1.1] - 2026-06-13

All changes are **additive**; no existing public API is removed or altered.

### Added

- `cmd::CommandIndex<A>` — command-palette index backed by a `BTreeMap<String, A>`.
  `bind(name, action)` (last-write-wins, returns the old value like `Keymap::bind`),
  `get(name) -> Option<&A>` for exact dispatch, `complete(prefix) -> impl Iterator`
  for front-prefix enumeration (lexicographic order is a specification, not an
  implementation detail), and `iter()` for full listing. Module-scoped so a future
  `keymap-cmd` crate extraction is non-destructive (module→crate via shim is
  reversible; the reverse is not).
- `validate_rebind<A>(layers, target, proposed, reserved) -> RebindVerdict<'_, A>` —
  opt-in advisory validator for runtime rebinding. Checks whether `proposed` collides
  with `reserved` keys (including legacy-terminal collisions via `legacy_form`) before
  any live mutation. **Contract note:** reserved chords are rejected unconditionally —
  this includes re-binding a key to the same action it already holds (strict, no
  same-action exception). The before/after semantic difference from a
  clone-simulate approach is documented here: the example's approach compared the
  resolved action; `validate_rebind` compares the chord itself against the reserved
  set and does not inspect the action value. Callers who want same-action re-binds to
  succeed should filter the reserved set themselves before calling. Not a gate on
  `Keymap::bind` — advisory only; `Keymap::bind` is unchanged.
- `RebindVerdict<'_, A>` (`#[non_exhaustive]`) — result of `validate_rebind`:
  `Allowed { legacy_form: LegacyForm }`, `Blocked { reason: BreakReason }`.
- `BreakReason` (`#[non_exhaustive]`) — reason a rebind was blocked:
  `Reserved { chord: KeyInput }`, `LegacyCollision { chord: KeyInput, collapses_to: KeyInput }`.

## [0.1.0] - 2026-05-28

### Added

- Initial public API: `Key`, `Modifiers`, `KeyInput` (with the `FromStr` /
  `Display` chord grammar and the `KeyInput::normalized` Shift-folding rule),
  and the generic binding table `Keymap<A>` keyed on a caller-defined action
  type `A`.
- `resolve_layered(layers, input)` for lexical-scope-style context resolution
  (first hit wins, miss falls outward), and `resolve_passthrough(layers, input,
  raw) -> Resolution` for PTY-aware misses that carry the original bytes back
  out as `Resolution::Passthrough`.
- `legacy_lints(&Keymap<A>) -> Vec<LegacyLint>` plus `KeyInput::legacy_form()`
  for the static, terminal-independent legacy-C0 survivability lower bound.
- Optional `crossterm` feature: `TryFrom<crossterm::event::KeyEvent> for
  KeyInput`. Gated so a crossterm major bump is not a `keymap-core` major bump
  for default builds.

[Unreleased]: https://github.com/S-Nakamur-a/keymap-rs/commits/main
[0.1.1]: https://crates.io/crates/keymap-core/0.1.1
[0.1.0]: https://crates.io/crates/keymap-core/0.1.0
