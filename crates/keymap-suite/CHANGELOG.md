# Changelog

All notable changes to `keymap-suite` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

## [0.1.1] - 2026-06-13

All changes are **additive** re-exports; no existing public API is removed or altered.

### Added

The following items are now re-exported from the suite root (not the prelude —
more specialised, so held at root until usage justifies prelude promotion):

- `CommandIndex<A>` — from `keymap_core::cmd`. Command-palette index; `bind`,
  `get`, `complete(prefix)`, `iter`.
- `TimedPending` / `TimedStep` — from `keymap-seq`. Deadline-aware sequence
  buffer wrapper.
- `validate_rebind` / `RebindVerdict` / `BreakReason` / `LegacyForm` — from
  `keymap-core`. `LegacyForm` is included because `RebindVerdict::Allowed`
  carries it (same precedent as `UnsupportedKey` for the crossterm adapter).
- `merge` / `Merged` / `MergeNote` — from `keymap-config`. Defaults⊕user overlay.
- `WarningKind` — from `keymap-config`. Fieldless discriminant for `Warning`;
  replaces per-consumer enum re-typing.
- `to_toml_layered_with_unbinds` — from `keymap-config`. Round-trip tombstone
  serializer.
- `legacy_lints` / `LegacyLint` — from `keymap-core`. Root re-export only (not
  prelude) — opt-in diagnostic pass, not a per-event operation. Previously only
  reachable by depending on `keymap-core` directly; now available without a
  direct `keymap-core` dependency. This gap was found by the `keymap-showcase`
  measurement run and closed in the same changeset.

### Release-flow note

As demonstrated by PR #1's CI log: a version-bump PR causes `cargo-semver-checks`
to skip all checks (0 checks / 252 skipped). The recommended release flow is
therefore: **(1)** land all feature changes on a bump-free PR so semver-checks
runs and validates additivity, then **(2)** land the version bump alone in a
separate follow-up PR. This 0.1.1 working-tree state reflects the final combined
form; when converting to commits, split at that boundary.

## [0.1.0] - 2026-05-29

### Added

- First real release, promoted from the `0.0.0` name-reservation placeholder.
  The crate is now the opinionated facade over `keymap-core` / `keymap-config`
  / `keymap-seq` for the nine-out-of-ten case (load TOML, resolve a key, drive
  the multi-key sequence buffer).
- `keys_for_action(&Keymap<A>, &A) -> Vec<&KeyInput>` — reverse lookup for help
  screens and which-key menus (the inverse of `Keymap::get`). Returns borrowed
  chords in unspecified order and does not format them, so display and sorting
  stay the caller's. Operates on one layer; map it over your active chain for
  layered discovery.
- `chords` module — terse constructors `key(c)` / `ctrl(c)` / `alt(c)` for the
  common modifier-plus-character case, so building a `KeyInput` in code reads
  `ctrl('s')` instead of `KeyInput::new(Key::Char('s'), Modifiers::CTRL)`. They
  funnel through `KeyInput::normalized` (same rule as the TOML grammar and the
  crossterm adapter), so a chord built in code matches one that arrives at
  runtime. Named keys / multi-modifier chords use `"ctrl+shift+f1".parse()` or
  `KeyInput::new`; a bare `shift(char)` is intentionally absent (a sole Shift on
  a character folds away, so it would equal `key(char)`). Kept namespaced
  (`chords::ctrl`) — the prelude re-exports the module, not the bare names,
  which are too collision-prone to flatten.
- Optional `crossterm` feature: turns on `keymap-core`'s `crossterm` feature
  (where `TryFrom<crossterm::event::KeyEvent> for KeyInput` lives) and
  re-exports `UnsupportedKey`. The default build stays backend-neutral. Only
  the `crossterm` feature name is reserved; termion/termwiz are not pre-empted.
- Curated re-exports from the underlying crates: `Key`, `KeyInput`,
  `Modifiers`, `Keymap`, `resolve_layered` (from `keymap-core`); `BuildError`,
  `Warning`, `GLOBAL_LAYER`, `to_toml`, `to_toml_layered` (from
  `keymap-config`); `SequenceKeymap`, `Match`, `Continuation`, `SeqBindError`,
  `PendingSequence`, `Step` (from `keymap-seq`). Re-exports are by selection,
  not by glob, so the suite's surface grows only when this crate does.
- `Loaded<A>` — a type alias for `keymap_config::BuildOutput<A>`. Suite users
  see one type name; later drops to `keymap-config` see the same value with
  no conversion.
- `LoadedExt` extension trait carrying suite-only conveniences:
  `deny_warnings(self) -> Result<Self, Vec<Warning>>` for the strict-mode
  escape hatch on a lenient loader, and `pending_sequence(&self) ->
  PendingSequence` as a discoverability shortcut for the partner type.
- `from_toml_str(toml, resolve) -> Result<Loaded<A>, BuildError>` — a thin
  rename over `keymap_config::from_str` so it sits beside `from_toml_path`
  under one vocabulary.
- `from_toml_path(path, resolve) -> Result<Loaded<A>, LoadError>` — reads a
  UTF-8 TOML file and parses it.
- `LoadError { Io(io::Error), Build(BuildError) }` (`#[non_exhaustive]`)
  distinguishing "the file could not be read" from "the contents are
  malformed", with `Display` / `Error` / `From<io::Error>` /
  `From<BuildError>`.
- `keymap_suite::prelude` — a one-import bundle of the nine-out-of-ten case
  vocabulary (`Key`, `KeyInput`, `Modifiers`, `Keymap`, `resolve_layered`,
  `SequenceKeymap`, `Match`, `PendingSequence`, `Step`, `Loaded`,
  `LoadedExt`, `keys_for_action`, the `chords` module, `Warning`, `BuildError`,
  `LoadError`).

### Notes

- This release's scope was set against a real consumer (the `conductor` TUI):
  its hand-written keymap glue showed the genuine boilerplate was *stateless*
  (reverse lookup, crossterm conversion), not stateful. So `keys_for_action`
  and the `crossterm` feature ship now; a stateful `Runtime` does not (the
  consumer uses no multi-key sequences, so it would go unused).
- Several candidates were considered and **intentionally deferred** until a
  second consumer or settled semantics justify them (see the workspace
  `docs/STATUS.md` for the trigger conditions):
  - a `#[derive(Action)]` macro — `strum` already covers name↔variant mapping;
    a keymap-specific derive waits for keymap-specific variant metadata.
  - a defaults-⊕-user `merge`/`overlay` helper — its semantics (silent
    override vs. conflict, unbind/tombstone expression) are undecided, and one
    consumer is not enough to fix them.
  - warning ergonomics (`Warning::kind()` / `chord()` accessors) — wait for a
    second consumer that re-types `Warning` to avoid the `#[non_exhaustive]`
    match.
- The same TOML loaded via `keymap-suite` and via `keymap-config` behaves
  identically by design — the suite never elevates a `Warning` to a
  `BuildError` internally.
- `keymap-term` is *not* included in the facade: PTY byte decoding is a
  niche use case and would widen the suite's dependency surface unhelpfully
  for the common TUI author.

[Unreleased]: https://github.com/S-Nakamur-a/keymap-rs/commits/main
[0.1.1]: https://crates.io/crates/keymap-suite/0.1.1
[0.1.0]: https://crates.io/crates/keymap-suite/0.1.0
