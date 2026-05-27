//! Invariants over the committed `captures/*.toml` fixtures.
//!
//! These run in CI with no terminal: once a capture is recorded by hand and
//! committed, this is the net that catches it rotting or contradicting itself.
//! It also mechanises the manual self-checks from `docs/TESTING.md` (the DA1
//! reply must end in `c`, "supported" must agree with the kitty reply), so they
//! no longer depend on a human eyeballing the file.

use std::fs;
use std::path::{Path, PathBuf};

use keymap_term::{Capture, from_hex};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The fixed key list the probe records. The source of truth is
/// `keymap-probe/src/main.rs` (`KEYS_TO_RECORD`); this is duplicated on purpose,
/// because the test crate cannot depend on the binary. The test asserts every
/// capture covers exactly this set, so a probe that silently stops recording a
/// key is caught. The enhanced re-capture (kitty flags) may *extend* this set —
/// when it lands, update the probe and this constant together.
const KEYS_TO_RECORD: &[&str] = &[
    "ctrl+a",
    "cmd+1",
    "alt+a",
    "ctrl+i",
    "shift+tab",
    "f1",
    "up",
    "down",
    "left",
    "right",
    "esc",
];

/// CSI final byte of a DA1 reply (`ESC [ ? ... c`). A complete capability query
/// response must end here; truncation means the read loop dropped bytes.
const DA1_FINAL: u8 = 0x63; // 'c'

fn captures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/keymap-term; captures/ lives at the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../captures")
        .canonicalize()
        .expect("captures/ directory should exist at the repo root")
}

fn load_captures() -> Vec<(String, Capture)> {
    let dir = captures_dir();
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).expect("captures/ should be readable") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).unwrap();
        let capture: Capture =
            toml::from_str(&text).unwrap_or_else(|e| panic!("{name}: malformed capture: {e}"));
        out.push((name, capture));
    }
    out
}

/// Mirror of `keymap-probe`'s `detect_kitty_support`: the first `ESC [ ?` CSI
/// reply terminating in `u` is the progressive-enhancement ack; `c` (DA1) means
/// the query was ignored. Kept in lockstep with the recorder so the stored
/// `value` and the stored `raw_response` cannot drift apart.
fn detect_kitty_support(response: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 < response.len() {
        if response[i] == 0x1b && response[i + 1] == b'[' && response[i + 2] == b'?' {
            for &b in &response[i + 3..] {
                if (0x40..=0x7e).contains(&b) {
                    return b == b'u';
                }
            }
            return false;
        }
        i += 1;
    }
    false
}

#[test]
fn at_least_one_capture_is_committed() {
    // The captures are fixtures, not optional scratch — a green run with zero of
    // them would be a silently useless test, the worst failure mode for a net.
    assert!(
        !load_captures().is_empty(),
        "no captures/*.toml found; the fixture matrix has gone missing"
    );
}

#[test]
fn every_hex_token_decodes() {
    for (name, cap) in load_captures() {
        for c in &cap.capability {
            from_hex(&c.raw_response)
                .unwrap_or_else(|e| panic!("{name}: capability {:?} raw_response: {e}", c.name));
        }
        for kp in &cap.keypress {
            from_hex(&kp.raw_bytes)
                .unwrap_or_else(|e| panic!("{name}: keypress {:?} raw_bytes: {e}", kp.intended));
        }
    }
}

#[test]
fn captured_at_is_rfc3339() {
    for (name, cap) in load_captures() {
        OffsetDateTime::parse(&cap.meta.captured_at, &Rfc3339)
            .unwrap_or_else(|e| panic!("{name}: captured_at {:?}: {e}", cap.meta.captured_at));
    }
}

#[test]
fn capability_query_responses_are_complete() {
    // The mechanised form of the manual DA1 self-check in docs/TESTING.md §0:
    // a non-empty query response must terminate in the DA1 final byte `c`.
    // A truncated reply means the poll/read path lost bytes.
    for (name, cap) in load_captures() {
        for c in &cap.capability {
            if c.raw_response.is_empty() {
                continue;
            }
            let bytes = from_hex(&c.raw_response).unwrap();
            assert_eq!(
                bytes.last().copied(),
                Some(DA1_FINAL),
                "{name}: capability {:?} response does not end in DA1 `c` (truncated read?): {:?}",
                c.name,
                c.raw_response
            );
        }
    }
}

#[test]
fn kitty_value_agrees_with_its_raw_response() {
    // "supported"/"unsupported" must be derivable from the bytes that justified
    // it, by the same rule the recorder used.
    for (name, cap) in load_captures() {
        for c in &cap.capability {
            if c.name != "kitty_keyboard_protocol" || c.raw_response.is_empty() {
                continue;
            }
            let bytes = from_hex(&c.raw_response).unwrap();
            let detected = detect_kitty_support(&bytes);
            let expected = match c.value.as_str() {
                "supported" => true,
                "unsupported" => false,
                other => panic!("{name}: unexpected kitty value {other:?}"),
            };
            assert_eq!(
                detected, expected,
                "{name}: kitty value {:?} disagrees with raw_response {:?}",
                c.value, c.raw_response
            );
        }
    }
}

#[test]
fn every_keypress_mode_is_known() {
    // A typo in the probe's recorded mode would silently make a keypress
    // un-decodable (`DecodeMode::from_capture_mode` returns `None`). Catch it at
    // the schema level. `kitty-enhanced` is the mode `keymap-probe --enhanced`
    // records; its decode arm (`DecodeMode::KittyEnhanced`) follows once the
    // enhanced captures exist.
    const KNOWN_MODES: &[&str] = &["baseline", "kitty-enhanced"];
    for (name, cap) in load_captures() {
        for kp in &cap.keypress {
            assert!(
                KNOWN_MODES.contains(&kp.mode.as_str()),
                "{name}: keypress {:?} has unknown mode {:?}",
                kp.intended,
                kp.mode
            );
        }
    }
}

#[test]
fn each_capture_covers_the_recorded_key_set() {
    // A probe that stops emitting a key (or emits an unexpected extra one) is a
    // regression in the recorder; catch it against the known list.
    for (name, cap) in load_captures() {
        let mut intended: Vec<&str> = cap.keypress.iter().map(|k| k.intended.as_str()).collect();
        intended.sort_unstable();
        let mut expected: Vec<&str> = KEYS_TO_RECORD.to_vec();
        expected.sort_unstable();
        assert_eq!(
            intended, expected,
            "{name}: recorded key set does not match KEYS_TO_RECORD"
        );
    }
}
