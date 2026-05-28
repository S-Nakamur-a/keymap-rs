//! # keymap-suite — reserved crate name (placeholder)
//!
//! This crate intentionally exports **no public API**. It exists only to hold
//! the `keymap-suite` name on crates.io so that, if the four-crate split
//! (`keymap-core` / `keymap-config` / `keymap-seq` / `keymap-term`) is ever
//! folded back into a single feature-gated crate, the facade can be published
//! under this name without first negotiating with whoever else happens to have
//! taken it. (The shorter `keymaps` was the original choice but is already
//! held by an unrelated crate at version 1.x.)
//!
//! See `docs/STATUS.md` ("Packaging retreat: the single-crate option") in the
//! workspace repository for the trigger conditions and the rationale.
//!
//! If you are looking for the actual library, depend on one of the four real
//! crates instead.

#![doc(html_root_url = "https://docs.rs/keymap-suite/0.0.0")]
