//! Replays the committed `captures/*.toml` through the decoder, each keypress in
//! the `DecodeMode` its capture recorded (`baseline` or `kitty-enhanced`).
//!
//! The oracle here is **hand-written**, not derived from the capture's
//! `expected_key`/`expected_mods`: those fields record what the operator
//! *intended to press*, which is a different question from what the bytes
//! *decode to*. Several fixtures diverge on purpose — `cmd+1` recorded zero
//! bytes (the OS ate it), `alt+a` arrives as a composed glyph on some terminals
//! — and that divergence is the empirical finding #2 exists to surface. So the
//! expectations below were written by reading the recorded bytes by hand; a
//! decoder regression diverges from them, and they cannot be tautological with
//! the decoder because nothing here calls the decoder to compute its own oracle.

use std::fs;
use std::path::{Path, PathBuf};

use keymap_core::{Key, KeyInput, LegacyForm, Modifiers};
use keymap_term::{Capture, DecodeMode, Decoded, decode, from_hex};

fn captures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../captures")
        .canonicalize()
        .expect("captures/ directory should exist at the repo root")
}

fn load_captures() -> Vec<(String, Capture)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(captures_dir()).expect("captures/ should be readable") {
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

fn ki(key: Key, mods: Modifiers) -> KeyInput {
    KeyInput::normalized(key, mods)
}

/// What a `(intended, raw bytes)` pair should decode to, written from reading
/// the captures by hand. See the module docs on why this is not derived from
/// `expected_key`.
enum Expect {
    /// Decodes to exactly this key, consuming every recorded byte.
    Key(KeyInput),
    /// No complete key from these bytes (the recorded `raw_bytes` is empty —
    /// the OS/multiplexer swallowed the press). Decodes to `Incomplete`.
    NoKey,
}

/// The hand-written oracle. Branches on the recorded bytes where a key has more
/// than one empirical form (the `alt+a` two camps; keys a multiplexer eats).
fn expected(intended: &str, raw: &[u8]) -> Expect {
    match intended {
        // Cmd is swallowed by the OS in baseline on every recorded terminal.
        "cmd+1" => Expect::NoKey,
        // Inside tmux, ctrl+a is the prefix key and alt+a is eaten too, so both
        // record empty bytes there.
        "ctrl+a" | "alt+a" if raw.is_empty() => Expect::NoKey,
        "ctrl+a" => Expect::Key(ki(Key::Char('a'), Modifiers::CTRL)),
        // alt+a splits three ways: ESC+a (Alt-as-Meta, baseline) and the
        // enhanced CSI-u form `ESC[97;3u` are both reachable as alt+a; a composed
        // glyph (Option-as-glyph, both modes on kitty/iTerm) is not.
        "alt+a" => match raw {
            [0x1b, 0x61] => Expect::Key(ki(Key::Char('a'), Modifiers::ALT)),
            [0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x33, 0x75] => {
                Expect::Key(ki(Key::Char('a'), Modifiers::ALT))
            }
            [0xc3, 0xa5] => Expect::Key(ki(Key::Char('å'), Modifiers::NONE)),
            other => panic!("unexpected alt+a bytes {other:02x?}"),
        },
        // ctrl+i splits by mode: baseline 0x09 is indistinguishable from Tab (the
        // Ambiguous case); kitty-enhanced sends `ESC[105;5u`, recovering the
        // distinct Char('i')+CTRL.
        "ctrl+i" => match raw {
            [0x09] => Expect::Key(ki(Key::Tab, Modifiers::NONE)),
            [0x1b, 0x5b, 0x31, 0x30, 0x35, 0x3b, 0x35, 0x75] => {
                Expect::Key(ki(Key::Char('i'), Modifiers::CTRL))
            }
            other => panic!("unexpected ctrl+i bytes {other:02x?}"),
        },
        "shift+tab" => Expect::Key(ki(Key::Tab, Modifiers::SHIFT)),
        "f1" => Expect::Key(ki(Key::F(1), Modifiers::NONE)),
        "up" => Expect::Key(ki(Key::Up, Modifiers::NONE)),
        "down" => Expect::Key(ki(Key::Down, Modifiers::NONE)),
        "left" => Expect::Key(ki(Key::Left, Modifiers::NONE)),
        "right" => Expect::Key(ki(Key::Right, Modifiers::NONE)),
        "esc" => Expect::Key(ki(Key::Esc, Modifiers::NONE)),
        other => panic!("no decode expectation for intended key {other:?}"),
    }
}

#[test]
fn every_keypress_decodes_as_the_handwritten_oracle_says() {
    for (name, cap) in load_captures() {
        for kp in &cap.keypress {
            let mode = DecodeMode::from_capture_mode(&kp.mode)
                .unwrap_or_else(|| panic!("{name}: unknown decode mode {:?}", kp.mode));
            let raw = from_hex(&kp.raw_bytes).unwrap();
            let got = decode(&raw, mode);
            match expected(&kp.intended, &raw) {
                Expect::Key(input) => assert_eq!(
                    got,
                    Decoded::Key {
                        input,
                        consumed: raw.len(),
                    },
                    "{name}: {} ({:02x?})",
                    kp.intended,
                    raw
                ),
                Expect::NoKey => assert_eq!(
                    got,
                    Decoded::Incomplete,
                    "{name}: {} expected no key from {:02x?}",
                    kp.intended,
                    raw
                ),
            }
        }
    }
}

#[test]
fn alt_a_divergence_is_locked_in() {
    // The reachability finding, made permanent: where alt+a arrives as a composed
    // glyph, it decodes to something that is NOT `alt+a` (so it is unreachable as
    // alt+a); where it arrives as ESC+a, it decodes to exactly `alt+a`.
    //
    // This pins only the `alt+a` instance. The general claim — "for every key,
    // decode(raw) == parse(intended) iff the chord is reachable" — needs the
    // reachability classifier, which is deferred to the next increment (#2b).
    // Until then, the `expected()` oracle above checks each key's decode against
    // a hand-written value, but nothing asserts that value equals `intended` for
    // the reachable keys; that biconditional is the classifier's job.
    let alt_a = "alt+a".parse::<KeyInput>().unwrap();
    let mut saw_composed = false;
    let mut saw_meta = false;
    let mut saw_csi_u = false;
    for (name, cap) in load_captures() {
        for kp in cap.keypress.iter().filter(|k| k.intended == "alt+a") {
            let raw = from_hex(&kp.raw_bytes).unwrap();
            if raw.is_empty() {
                continue; // tmux ate it; covered by the oracle test
            }
            let mode = DecodeMode::from_capture_mode(&kp.mode)
                .unwrap_or_else(|| panic!("{name}: unknown decode mode {:?}", kp.mode));
            let Decoded::Key { input, .. } = decode(&raw, mode) else {
                panic!("{name}: alt+a {raw:02x?} should decode to a key");
            };
            match raw.as_slice() {
                [0xc3, 0xa5] => {
                    assert_ne!(input, alt_a, "{name}: composed glyph must NOT be alt+a");
                    saw_composed = true;
                }
                [0x1b, 0x61] => {
                    assert_eq!(input, alt_a, "{name}: ESC+a must be reachable as alt+a");
                    saw_meta = true;
                }
                // Enhanced CSI-u: alt+a arrives as a real `alt+a` (mod 3 = 1+alt).
                [0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x33, 0x75] => {
                    assert_eq!(
                        input, alt_a,
                        "{name}: enhanced CSI-u must be reachable as alt+a"
                    );
                    saw_csi_u = true;
                }
                other => panic!("{name}: unexpected alt+a bytes {other:02x?}"),
            }
        }
    }
    // Each camp must be represented, or this test silently proves nothing.
    assert!(saw_composed, "no Option-composes capture present");
    assert!(saw_meta, "no Alt-as-Meta capture present");
    assert!(saw_csi_u, "no enhanced CSI-u alt+a capture present");
}

#[test]
fn decode_never_panics_on_any_recorded_bytes() {
    // Every recorded byte string — keypresses *and* capability query replies —
    // is real wire input; the decoder must terminate on all of it without
    // panicking, whatever the verdict.
    for (_, cap) in load_captures() {
        // Both modes: the CSI-u parser's numeric boundaries (overflow, surrogate)
        // are exercised most by running the CSI-shaped capability replies through
        // the enhanced arm too.
        for mode in [DecodeMode::Baseline, DecodeMode::KittyEnhanced] {
            for kp in &cap.keypress {
                let _ = decode(&from_hex(&kp.raw_bytes).unwrap(), mode);
            }
            for c in &cap.capability {
                let _ = decode(&from_hex(&c.raw_response).unwrap(), mode);
            }
        }
    }
}

#[test]
fn empty_input_is_incomplete() {
    assert_eq!(decode(&[], DecodeMode::Baseline), Decoded::Incomplete);
}

#[test]
fn c0_decode_agrees_with_legacy_form() {
    // For every ctrl+<letter>, the byte it sends must decode to whatever
    // `legacy_form` says it collapses to (or to itself when representable). This
    // keeps the empirical decoder and the static lower bound from drifting apart.
    for c in 'a'..='z' {
        let chord = KeyInput::new(Key::Char(c), Modifiers::CTRL);
        let byte = (c as u8) & 0x1f;
        let want = match chord.legacy_form() {
            LegacyForm::Representable => chord,
            LegacyForm::CollapsesTo(x) => x,
            LegacyForm::Unrepresentable => unreachable!("ctrl+{c} is C0-representable"),
        };
        assert_eq!(
            decode(&[byte], DecodeMode::Baseline),
            Decoded::Key {
                input: want,
                consumed: 1,
            },
            "ctrl+{c} (byte {byte:#04x}) disagrees with legacy_form",
        );
    }
}

#[test]
fn enhanced_recovers_what_c0_collapses_for_ctrl_i() {
    // The mirror of `c0_decode_agrees_with_legacy_form`: where baseline C0 *loses*
    // a distinction (ctrl+i collapses onto Tab), the kitty-enhanced encoding
    // *recovers* it. ctrl+i is the one collapsing chord with a captured enhanced
    // byte form (`ESC[105;5u`); ctrl+m / ctrl+[ also collapse statically, but
    // their enhanced bytes are unmeasured, so we do not invent an oracle.
    let ctrl_i = KeyInput::new(Key::Char('i'), Modifiers::CTRL);
    // The static lower bound still says ctrl+i is lost under legacy C0 …
    let LegacyForm::CollapsesTo(collapsed) = ctrl_i.legacy_form() else {
        panic!("ctrl+i should collapse onto Tab under legacy C0");
    };
    assert_eq!(collapsed, KeyInput::new(Key::Tab, Modifiers::NONE));
    // … yet the enhanced decoder returns the original chord, not the collapse.
    let enhanced = decode(
        &[0x1b, 0x5b, 0x31, 0x30, 0x35, 0x3b, 0x35, 0x75],
        DecodeMode::KittyEnhanced,
    );
    assert_eq!(
        enhanced,
        Decoded::Key {
            input: ctrl_i,
            consumed: 8,
        },
    );
    assert_ne!(
        enhanced,
        Decoded::Key {
            input: collapsed,
            consumed: 8,
        },
        "enhanced ctrl+i must not collapse onto Tab",
    );
}
