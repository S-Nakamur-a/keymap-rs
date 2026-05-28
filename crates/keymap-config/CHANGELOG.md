# Changelog

All notable changes to `keymap-config` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate follows the
[Cargo pre-1.0 SemVer interpretation](https://doc.rust-lang.org/cargo/reference/semver.html)
(`0.MINOR.PATCH`, where a MINOR bump is breaking).

> When cutting `0.1.0`, move the entries below under a new
> `## [0.1.0] - YYYY-MM-DD` heading and reset `[Unreleased]` to empty.

## [Unreleased]

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
