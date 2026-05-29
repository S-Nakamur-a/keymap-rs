# Changelog

All notable changes to `keymap-suite` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

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
  `LoadedExt`, `keys_for_action`, `Warning`, `BuildError`, `LoadError`).

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
[0.1.0]: https://crates.io/crates/keymap-suite/0.1.0
