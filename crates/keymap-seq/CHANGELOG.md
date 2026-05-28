# Changelog

All notable changes to `keymap-seq` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

## [0.1.0] - 2026-05-28

### Added

- `SequenceKeymap<A>` for prefix-free multi-chord bindings (leader keys,
  emacs-style `ctrl+x ctrl+s`). `bind` rejects prefix-shadow collisions at
  build time with `SeqBindError::PrefixShadow`, keeping `lookup` total and
  time-free.
- `SequenceKeymap::lookup(&[KeyInput]) -> Match<'_, A>` with the exhaustive
  `Match { Exact(&A), Prefix, NoMatch }`; `continuations(&[KeyInput])` yields
  next-step `Continuation { Action(&A), Prefix }` pairs for which-key style
  menus; `bindings()` enumerates every leaf for discovery.
- `PendingSequence` and `Step` — a small caller-owned helper that folds the
  pending-key buffer and the idle-flush step into one place so the example loop
  stays a few lines. The pending buffer and any inter-key timeout still live
  caller-side; the table itself holds no clock.

[Unreleased]: https://github.com/S-Nakamur-a/keymap-rs/commits/main
[0.1.0]: https://crates.io/crates/keymap-seq/0.1.0
