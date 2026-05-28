# keymap-suite (reserved placeholder)

This crate is a **name reservation**, not a usable library. It exists only to
hold the `keymap-suite` name on crates.io. (The shorter `keymaps` was the
original choice, but is already held by an unrelated crate at version 1.x.)

The real library lives across four sibling crates:

- [`keymap-core`](https://crates.io/crates/keymap-core) — neutral key vocabulary and `Keymap<A>` lookup
- [`keymap-config`](https://crates.io/crates/keymap-config) — TOML-driven bindings with conflict warnings
- [`keymap-seq`](https://crates.io/crates/keymap-seq) — prefix-free multi-key sequences (leader keys, `ctrl+x ctrl+s`)
- [`keymap-term`](https://crates.io/crates/keymap-term) — measurement-first byte decoder for raw terminal bytes

See the [workspace repository](https://github.com/S-Nakamur-a/keymap-rs) for the design rationale; see `docs/STATUS.md` ("Packaging retreat: the single-crate option") for why this name is held in reserve.

If the four-crate split is ever folded back into a single feature-gated crate (`features = ["config", "seq", "term", "crossterm"]`, in the shape of `tokio`'s feature gating), the facade will be published under this name. Until then, depend on one of the four real crates above.
