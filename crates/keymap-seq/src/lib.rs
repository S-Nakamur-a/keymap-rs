//! # keymap-seq
//!
//! Multi-key *sequence* resolution layered on top of [`keymap-core`]'s
//! single-chord vocabulary: leader trees (`space w`), emacs-style prefix chords
//! (`ctrl+x ctrl+s`), and the like.
//!
//! Like [`keymap-core`], this crate is **state-free**. A [`SequenceKeymap`]
//! answers "given the keys pressed so far, is this an exact binding, a prefix of
//! a longer one, or neither?" via [`SequenceKeymap::lookup`], and "what keys may
//! follow this prefix?" via [`SequenceKeymap::continuations`] (for which-key-style
//! menus). The *pending key buffer* — the keys typed since the last resolution —
//! lives in the caller,
//! exactly as `keymap_core::resolve_layered` keeps the active-layer choice
//! caller-side. The library holds no clock and no mutable session state.
//!
//! ## The prefix-free invariant
//!
//! A binding may not be a proper prefix of another binding. With timeouts out of
//! scope (see below), `j` bound *and* `j j` bound would make the press of `j`
//! undecidable — fire `j` now, or wait for the second `j`? Only a clock or a
//! lookahead could break that tie, and this crate has neither. So
//! [`SequenceKeymap::bind`] rejects such a pair with [`SeqBindError::PrefixShadow`],
//! which keeps `lookup` a *total, deterministic, time-free* function: a prefix is
//! always [`Match::Prefix`], a leaf is always [`Match::Exact`], anything off the
//! tree is [`Match::NoMatch`].
//!
//! ## Timeouts and the caller loop
//!
//! `jj`-style "press twice quickly" bindings need a time window, which is a
//! property of the caller's event loop (when to read the clock, blocking vs
//! `poll`-with-deadline), not of this table. The intended pattern keeps the
//! library time-free. [`PendingSequence`] folds the buffer bookkeeping below into
//! a small caller-owned helper (`feed` to push a key, `flush` for the idle
//! timeout); the raw `lookup` loop it wraps is:
//!
//! ```
//! use keymap_core::{Key, KeyInput, Modifiers};
//! use keymap_seq::{Match, SequenceKeymap};
//!
//! # #[derive(Debug, PartialEq)]
//! enum Action { Save }
//!
//! let cx = KeyInput::new(Key::Char('x'), Modifiers::CTRL);
//! let cs = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
//! let mut map = SequenceKeymap::new();
//! map.bind([cx, cs], Action::Save).unwrap();
//!
//! // The caller owns the pending buffer.
//! let mut pending: Vec<KeyInput> = Vec::new();
//! for key in [cx, cs] {
//!     pending.push(key);
//!     match map.lookup(&pending) {
//!         Match::Exact(action) => {
//!             assert_eq!(action, &Action::Save);
//!             pending.clear(); // fired — reset for the next sequence
//!         }
//!         Match::Prefix => { /* keep waiting (and, for `jj`, start a timer) */ }
//!         Match::NoMatch => { pending.clear(); /* replay/pass through */ }
//!     }
//! }
//! ```
//!
//! ## Runnable example
//!
//! `cargo run -p keymap-seq --example leader_sequence` runs the
//! buffer-and-flush loop above, including a timed `jj`-style window demo
//! ([source](https://github.com/S-Nakamur-a/keymap-rs/tree/main/crates/keymap-seq/examples/leader_sequence.rs)).
//!
//! [`keymap-core`]: keymap_core

mod map;
mod progress;

pub use map::{Continuation, Match, SeqBindError, SequenceKeymap};
pub use progress::{PendingSequence, Step};
