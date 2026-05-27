//! The empirical reachability layer, checked against the committed
//! `captures/*.toml` fixtures (headless, CI-safe).
//!
//! Where the unit tests in `src/reachability.rs` pin the *logic* on hand-built
//! captures, these pin that the logic stays consistent with *real* recorded
//! terminal behaviour — most importantly the `alt+a` two-camp divergence the
//! decoder's `decode_fixtures.rs` also locks in.

use std::fs;
use std::path::{Path, PathBuf};

use keymap_core::KeyInput;
use keymap_term::{Capture, DecodeMode, Decoded, Reachability, decode, from_hex, reachability};

fn load_captures() -> Vec<(String, Capture)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../captures")
        .canonicalize()
        .expect("captures/ directory should exist at the repo root");
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).expect("captures/ should be readable") {
        let path: PathBuf = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).unwrap();
        let capture: Capture =
            toml::from_str(&text).unwrap_or_else(|e| panic!("{name}: malformed capture: {e}"));
        out.push((name, capture));
    }
    assert!(!out.is_empty(), "expected committed captures");
    out
}

/// The reachability verdict for `chord` in `capture`, if recorded.
fn verdict(capture: &Capture, chord: &KeyInput) -> Option<Reachability> {
    reachability(capture)
        .into_iter()
        .find(|(k, _)| k == chord)
        .map(|(_, v)| v)
}

#[test]
fn reachable_iff_decode_equals_intended_across_all_captures() {
    // The law, verified independently of the function: recompute `decode ==
    // intended` from the raw bytes and assert it matches `Reachable`-ness, for
    // every recorded press in every committed capture (both directions).
    for (name, capture) in load_captures() {
        for keypress in &capture.keypress {
            let Ok(intended) = keypress.intended.parse::<KeyInput>() else {
                continue;
            };
            let Some(mode) = DecodeMode::from_capture_mode(&keypress.mode) else {
                continue;
            };
            let bytes = from_hex(&keypress.raw_bytes).unwrap();
            let decode_matches =
                matches!(decode(&bytes, mode), Decoded::Key { input, .. } if input == intended);

            let is_reachable =
                matches!(verdict(&capture, &intended), Some(Reachability::Reachable));
            assert_eq!(
                decode_matches, is_reachable,
                "{name}: biconditional broken for {}",
                keypress.intended
            );
        }
    }
}

#[test]
fn alt_a_two_camp_divergence_shows_up_in_reachability() {
    // The empirical refinement that justifies this layer: `alt+a` is statically
    // Representable, yet some terminals make it reachable (Meta `1b 61`) and
    // others do not (composed glyph `c3 a5`). Both camps must appear, or the
    // verdict has silently collapsed to a single answer.
    let alt_a: KeyInput = "alt+a".parse().unwrap();
    let mut saw_reachable = false;
    let mut saw_unreachable = false;
    for (_, capture) in load_captures() {
        match verdict(&capture, &alt_a) {
            Some(Reachability::Reachable) => saw_reachable = true,
            Some(Reachability::Unreachable { .. }) => saw_unreachable = true,
            // `Reachability` is #[non_exhaustive]; an unmodelled future verdict
            // counts as neither camp.
            _ => {}
        }
    }
    assert!(
        saw_reachable,
        "expected a capture where alt+a is reachable (Meta)"
    );
    assert!(
        saw_unreachable,
        "expected a capture where alt+a is unreachable (composed glyph / eaten)"
    );
}
