# Changelog

All notable changes to `keymap-term` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

## [Unreleased]

## [0.1.0] - 2026-05-28

### Added

- `Capture` schema for recorded terminal sessions (`Meta`, `Capability`,
  `KeyPress`) plus the lossless hex codec `to_hex` / `from_hex` for storing raw
  bytes safely in TOML. Fixtures live under `captures/` at the workspace root.
- `decode(&[u8], DecodeMode) -> Decoded` pure byte decoder built from the
  committed captures. Returns `Decoded { Key { input, consumed }, Incomplete,
  Unrecognized }`; `consumed` lets a streaming caller advance its own buffer.
  Decoded keys are *untrusted* (a PTY child can forge any byte sequence) — no
  reserved-key filtering happens here; that stays a resolve-time concern.
- `DecodeMode { Baseline, KittyEnhanced }` (both `#[non_exhaustive]`); the
  kitty arm recognises CSI-u sequences that disambiguate e.g. `ctrl+i` from
  `Tab`.
- `reachability(&Capture) -> Vec<(KeyInput, Reachability)>` reports the
  *empirical* per-terminal reachability of recorded chords (`Reachable`,
  `Unreachable { decoded }`), refining `keymap_core::legacy_form`'s static
  lower bound. Absence from the returned list is itself the "no evidence" case.

[Unreleased]: https://github.com/S-Nakamur-a/keymap-rs/commits/main
[0.1.0]: https://crates.io/crates/keymap-term/0.1.0
