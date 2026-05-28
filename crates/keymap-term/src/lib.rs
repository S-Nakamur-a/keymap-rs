//! # keymap-term — capture schema and capability-aware decoder
//!
//! `keymap-rs` is *measurement first*: before any capability-aware byte decoding
//! is written, we record what real terminals actually send. This crate defines
//! the on-disk schema for those recordings ([`Capture`]) plus the lossless hex
//! codec used to store raw bytes safely in TOML.
//!
//! A capture is provenance-bearing ground truth: which terminal, whether tmux or
//! SSH was in the path, when it was taken, what each key produced on the wire,
//! and how each capability value was learned. Captures live under `captures/`,
//! one file per terminal/session, and are the fixtures the decoder is validated
//! against. The interactive recorder is the `keymap-probe` binary.
//!
//! The [`decode`] function turns recorded bytes back into `keymap_core::KeyInput`
//! under a [`DecodeMode`], returning a [`Decoded`] outcome. It is a pure,
//! state-free function built only from the byte shapes the committed captures
//! contain (baseline and kitty-enhanced today).
//!
//! On top of it, [`reachability`] reports *empirical* per-terminal reachability:
//! for one [`Capture`] it enumerates which recorded chords actually arrive as
//! themselves (the empirical refinement of `keymap_core::legacy_form`'s static
//! lower bound). This crate carries no terminal-I/O dependencies.
//!
//! ## Headless verification
//!
//! There is no `examples/` directory for this crate; the decoder is verified
//! against the committed `captures/*.toml` fixtures by `tests/decode_fixtures.rs`,
//! `tests/capture_invariants.rs`, and `tests/reachability_fixtures.rs`
//! (`cargo test -p keymap-term`). New decoder behaviour is added by extending
//! the captures, not by writing speculative byte shapes.

mod decode;
mod reachability;

pub use decode::{DecodeMode, Decoded, decode};
pub use reachability::{Reachability, reachability};

use serde::{Deserialize, Serialize};

/// A single recording session against one terminal environment.
///
/// This is a serialization DTO: new fields are added with `#[serde(default)]`
/// for data-level backward compatibility, so it is intentionally *not*
/// `#[non_exhaustive]` — callers (the recorder) construct it directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    /// Environment metadata describing where and when this was recorded.
    pub meta: Meta,
    /// Capability values observed (e.g. kitty keyboard protocol support).
    #[serde(default)]
    pub capability: Vec<Capability>,
    /// The keypresses recorded, with the raw bytes each produced.
    #[serde(default)]
    pub keypress: Vec<KeyPress>,
}

/// Where and when a [`Capture`] was taken — the index used to tell captures
/// apart and to judge when one has gone stale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    /// RFC 3339 timestamp of when the capture was taken (freshness signal).
    pub captured_at: String,
    /// Version of the recorder that produced this file (format-migration hook).
    pub harness_version: String,
    /// The `$TERM` value, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    /// The `$TERM_PROGRAM` value, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term_program: Option<String>,
    /// The `$TERM_PROGRAM_VERSION` value, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term_program_version: Option<String>,
    /// Whether a tmux/screen multiplexer was in the path (changes encodings).
    pub tmux: bool,
    /// Whether the session was over SSH (the far terminal's identity is hidden).
    pub ssh: bool,
}

/// How a capability value was learned. The same value is worth far less if it
/// came from an environment-variable guess than from a query response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Provenance {
    /// Learned from the terminal's reply to an active query (most reliable).
    QueryResponse,
    /// Inferred from an environment variable (a hint, not a contract).
    EnvVar,
    /// Entered by a human operator.
    Manual,
}

/// One observed terminal capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Capability name, e.g. `"kitty_keyboard_protocol"`.
    pub name: String,
    /// The observed value, e.g. `"supported"` / `"unsupported"`.
    pub value: String,
    /// How `value` was learned.
    pub provenance: Provenance,
    /// The raw response bytes (hex), when learned from a query.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw_response: Vec<String>,
}

/// One recorded keypress: what the operator intended versus what arrived.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPress {
    /// Human label of the key the operator was asked to press, e.g. `"cmd+1"`.
    pub intended: String,
    /// The capability mode the terminal was in, e.g. `"baseline"`.
    pub mode: String,
    /// Raw bytes received on the wire (hex), the primary datum.
    pub raw_bytes: Vec<String>,
    /// Sizes of each `read()` chunk, preserving where reads split a sequence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_chunks: Vec<usize>,
    /// `KeyInput` the `intended` label parsed to, e.g. `"Char('1')"`. The
    /// *expected* key; comparing it to `raw_bytes` reveals non-delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_key: Option<String>,
    /// Modifiers the `intended` label parsed to, e.g. `["SUPER"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_mods: Option<Vec<String>>,
}

/// Encodes bytes as lowercase two-digit hex strings (`0x1b` -> `"1b"`).
///
/// Raw terminal bytes include control characters and non-UTF-8 sequences, so
/// they are stored as hex rather than as a string to keep captures lossless and
/// human-diffable.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> Vec<String> {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Error returned when a hex string in a capture cannot be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseHexError {
    token: String,
}

impl core::fmt::Display for ParseHexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "invalid hex byte token: {:?}", self.token)
    }
}

impl std::error::Error for ParseHexError {}

/// Decodes hex strings produced by [`to_hex`] back into bytes.
///
/// # Errors
///
/// Returns [`ParseHexError`] if any token is not exactly two hex digits.
pub fn from_hex(hex: &[String]) -> Result<Vec<u8>, ParseHexError> {
    hex.iter()
        .map(|tok| {
            if tok.len() == 2 {
                u8::from_str_radix(tok, 16).map_err(|_| ParseHexError { token: tok.clone() })
            } else {
                Err(ParseHexError { token: tok.clone() })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_including_control_and_high_bytes() {
        let bytes = vec![0x00, 0x1b, 0x5b, 0x41, 0x7f, 0xff];
        let hex = to_hex(&bytes);
        assert_eq!(hex, ["00", "1b", "5b", "41", "7f", "ff"]);
        assert_eq!(from_hex(&hex).unwrap(), bytes);
    }

    #[test]
    fn from_hex_rejects_malformed_tokens() {
        assert!(from_hex(&["1".to_string()]).is_err());
        assert!(from_hex(&["zz".to_string()]).is_err());
        assert!(from_hex(&["1bb".to_string()]).is_err());
    }

    #[test]
    fn capture_round_trips_through_toml() {
        let capture = Capture {
            meta: Meta {
                captured_at: "2026-05-26T01:00:00Z".to_string(),
                harness_version: "0.1.0".to_string(),
                term: Some("xterm-256color".to_string()),
                term_program: Some("iTerm.app".to_string()),
                term_program_version: Some("3.5.0".to_string()),
                tmux: false,
                ssh: false,
            },
            capability: vec![Capability {
                name: "kitty_keyboard_protocol".to_string(),
                value: "unsupported".to_string(),
                provenance: Provenance::QueryResponse,
                raw_response: to_hex(&[0x1b, 0x5b, 0x3f, 0x63]),
            }],
            keypress: vec![KeyPress {
                intended: "cmd+1".to_string(),
                mode: "baseline".to_string(),
                raw_bytes: to_hex(&[0x31]),
                read_chunks: vec![1],
                expected_key: Some("Char('1')".to_string()),
                expected_mods: Some(vec!["SUPER".to_string()]),
            }],
        };

        let serialized = toml::to_string(&capture).unwrap();
        let parsed: Capture = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.meta.term_program.as_deref(), Some("iTerm.app"));
        assert_eq!(parsed.capability[0].provenance, Provenance::QueryResponse);
        assert_eq!(parsed.keypress[0].intended, "cmd+1");
        assert_eq!(from_hex(&parsed.keypress[0].raw_bytes).unwrap(), [0x31]);
    }
}
