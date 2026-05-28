# Changelog

All notable changes to `keymap-core` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

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
[0.1.0]: https://crates.io/crates/keymap-core/0.1.0
