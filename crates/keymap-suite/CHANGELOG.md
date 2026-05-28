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
  `LoadedExt`, `Warning`, `BuildError`, `LoadError`).

### Notes

- This release is *deliberately small*. A stateful `Runtime`, an
  `#[derive(Action)]` macro, and a rebind sugar API were all considered and
  intentionally deferred until usage signal arrives (see the workspace
  `docs/STATUS.md`). The same TOML loaded via `keymap-suite` and via
  `keymap-config` behaves identically by design — the suite never elevates a
  `Warning` to a `BuildError` internally.
- `keymap-term` is *not* included in the facade: PTY byte decoding is a
  niche use case and would widen the suite's dependency surface unhelpfully
  for the common TUI author.

[Unreleased]: https://github.com/S-Nakamur-a/keymap-rs/commits/main
[0.1.0]: https://crates.io/crates/keymap-suite/0.1.0
