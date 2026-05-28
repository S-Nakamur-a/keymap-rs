//! # keymap-core
//!
//! The pure, state-free core of the `keymap-rs` family: a normalized key-input
//! vocabulary ([`Key`], [`Modifiers`], [`KeyInput`]) and a generic binding table
//! ([`Keymap`]) that maps inputs to a caller-defined action type `A`.
//!
//! This crate deliberately knows nothing about *terminals*, *config files*, or
//! *rendering*. Those live in sibling crates (`keymap-term`, `keymap-config`,
//! `keymap-ratatui`). The action type is never defined here — you bring your own
//! `A` (typically an `enum`), so the library never pulls your domain into itself.
//!
//! ## Phase 0 scope
//!
//! Today this crate offers single-key resolution by lookup:
//!
//! ```
//! use keymap_core::{Key, KeyInput, Keymap, Modifiers};
//!
//! #[derive(Clone, PartialEq, Debug)]
//! enum Action { Quit, Save }
//!
//! let mut map = Keymap::new();
//! map.bind(KeyInput::new(Key::Char('q'), Modifiers::CTRL), Action::Quit);
//!
//! let hit = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
//! assert_eq!(map.get(&hit), Some(&Action::Quit));
//!
//! // A miss is an absence, not an error: the caller passes the key through.
//! let miss = KeyInput::new(Key::Char('x'), Modifiers::NONE);
//! assert_eq!(map.get(&miss), None);
//! ```
//!
//! For a terminal *multiplexer*, [`resolve_passthrough`] is the raw-byte-carrying
//! sibling of [`resolve_layered`]: a miss carries the original bytes out as
//! [`Resolution::Passthrough`] for verbatim forwarding to the PTY sink, rather
//! than collapsing to `None`. Terminal-capability-aware reachability diagnostics
//! and multi-key prefix sequences arrive in sibling crates (`keymap-term`,
//! `keymap-seq`). [`Resolution`] is deliberately *exhaustive* (its three
//! dispositions are the complete set), so unlike most public types here it is not
//! `#[non_exhaustive]` — see its docs.
//!
//! ## Runnable examples
//!
//! Each scenario above has a runnable counterpart under
//! [`examples/`](https://github.com/S-Nakamur-a/keymap-rs/tree/main/crates/keymap-core/examples):
//! `basic_lookup`, `modal_keymap`, `discovery`, `ex_command`. Run e.g.
//! `cargo run -p keymap-core --example modal_keymap`. The examples are
//! exercised by `cargo test --workspace`, so they cannot silently drift from
//! the API.

mod input;
mod keymap;
mod legacy;
mod passthrough;

pub use input::{Key, KeyInput, Modifiers, ParseKeyInputError};
pub use keymap::{Keymap, resolve_layered};
pub use legacy::{LegacyForm, LegacyLint, legacy_lints};
pub use passthrough::{RawInput, Resolution, resolve_passthrough};

#[cfg(feature = "crossterm")]
pub use input::crossterm::UnsupportedKey;
