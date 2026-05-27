//! Empirical per-terminal reachability, refining the static legacy lower bound.
//!
//! `keymap_core::KeyInput::legacy_form` is a *terminal-independent* static fact
//! (what a chord loses under legacy C0 no matter the terminal). This module is
//! its empirical counterpart: for one recorded [`Capture`], it reports whether
//! each chord the terminal was asked to type actually *arrives as itself* — by
//! decoding the bytes the terminal really sent and comparing to what was
//! intended. The headline divergence it captures: `alt+a` is structurally
//! `Representable`, yet on some terminals it decodes to the composed glyph `å`
//! (so it is unreachable *as* `alt+a` there) while on others it arrives as a real
//! Meta `alt+a`.
//!
//! # Enumeration, not an oracle
//!
//! The captures are sparse — a fixed ~11-key list per terminal (see
//! `docs/TESTING.md`). A per-chord function `reachable(chord, capture)` would
//! therefore answer "no evidence" for almost its entire domain, an API that
//! overpromises empirical knowledge it does not have. So this is shaped like
//! `keymap_core::legacy_lints`: [`reachability`] *enumerates* the chords a
//! capture actually witnessed and returns a verdict for each. **Absence from the
//! result is the "no evidence" case** — structurally, with no variant to mistake
//! for a verdict, and with no silent fallback to the static `legacy_form` (a
//! caller wanting the static bound for an unwitnessed chord calls `legacy_form`
//! itself, consciously).
//!
//! # The reachability law
//!
//! For every entry returned, `verdict == Reachable` **iff** the recorded bytes
//! decode (under the capture's mode) to exactly the intended chord. An eaten
//! press (empty bytes, e.g. `cmd+1` swallowed by the OS, or `ctrl+a` by tmux)
//! decodes to [`Decoded::Incomplete`] and is [`Unreachable`], carrying that
//! `Decoded` as the evidence of *why*.
//!
//! [`Unreachable`]: Reachability::Unreachable

use keymap_core::KeyInput;

use crate::{Capture, DecodeMode, Decoded, decode, from_hex};

/// The empirical reachability verdict for one recorded chord on one terminal.
///
/// `#[non_exhaustive]` because the empirical outcome set is open (mirroring
/// [`Decoded`]): once captures contain a genuine on-the-wire collision between
/// two distinct intended chords (the empirical analogue of
/// `LegacyForm::CollapsesTo` — e.g. `ctrl+i` and `Tab` both sending `0x09`), an
/// `Ambiguous` variant can be added without breaking callers. The committed
/// fixtures contain no such non-empty collision today, so it is not modelled yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[must_use]
pub enum Reachability {
    /// The recorded bytes decode back to the intended chord: typing it on this
    /// terminal produces it. (The positive side of the reachability law.)
    Reachable,
    /// The recorded bytes decode to something *other* than the intended chord —
    /// a different key, or no complete key at all (an empty/eaten press →
    /// [`Decoded::Incomplete`], or [`Decoded::Unrecognized`]). The `decoded`
    /// outcome is the evidence.
    Unreachable {
        /// What the intended chord's recorded bytes actually decoded to.
        decoded: Decoded,
    },
}

/// Enumerates the empirical reachability of every chord `capture` recorded.
///
/// Each item pairs the intended [`KeyInput`] with its [`Reachability`] verdict,
/// sorted by canonical chord string for stable output. A recorded keypress is
/// skipped (contributes no entry) when its `intended` label does not parse, its
/// `mode` maps to no [`DecodeMode`], or its `raw_bytes` are not valid hex — such
/// a row is unusable evidence, indistinguishable from absence (the committed
/// fixtures are guarded against these by `tests/capture_invariants.rs`).
///
/// See the [module docs](self) for the enumeration rationale and the law.
#[must_use]
pub fn reachability(capture: &Capture) -> Vec<(KeyInput, Reachability)> {
    let mut out: Vec<(KeyInput, Reachability)> = Vec::new();
    for keypress in &capture.keypress {
        let Ok(intended) = keypress.intended.parse::<KeyInput>() else {
            continue;
        };
        let Some(mode) = DecodeMode::from_capture_mode(&keypress.mode) else {
            continue;
        };
        let Ok(bytes) = from_hex(&keypress.raw_bytes) else {
            continue;
        };

        let decoded = decode(&bytes, mode);
        let verdict = match decoded {
            Decoded::Key { input, .. } if input == intended => Reachability::Reachable,
            _ => Reachability::Unreachable { decoded },
        };
        out.push((intended, verdict));
    }
    out.sort_by_key(|(chord, _)| chord.to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyPress, Meta, to_hex};
    use keymap_core::{Key, Modifiers};

    fn meta() -> Meta {
        Meta {
            captured_at: "2026-05-26T00:00:00Z".to_string(),
            harness_version: "test".to_string(),
            term: None,
            term_program: None,
            term_program_version: None,
            tmux: false,
            ssh: false,
        }
    }

    /// A capture with the given `(intended, mode, raw_bytes)` keypresses.
    fn capture(presses: &[(&str, &str, &[u8])]) -> Capture {
        Capture {
            meta: meta(),
            capability: vec![],
            keypress: presses
                .iter()
                .map(|&(intended, mode, bytes)| KeyPress {
                    intended: intended.to_string(),
                    mode: mode.to_string(),
                    raw_bytes: to_hex(bytes),
                    read_chunks: vec![],
                    expected_key: None,
                    expected_mods: None,
                })
                .collect(),
        }
    }

    fn verdict(capture: &Capture, chord: &str) -> Option<Reachability> {
        let target: KeyInput = chord.parse().unwrap();
        reachability(capture)
            .into_iter()
            .find(|(k, _)| *k == target)
            .map(|(_, v)| v)
    }

    #[test]
    fn round_tripping_bytes_are_reachable() {
        // `ctrl+a` -> 0x01 -> decodes back to ctrl+a.
        let cap = capture(&[("ctrl+a", "baseline", &[0x01])]);
        assert_eq!(verdict(&cap, "ctrl+a"), Some(Reachability::Reachable));
    }

    #[test]
    fn bytes_decoding_to_a_different_key_are_unreachable() {
        // On a terminal that composes Option+a into `å`, `alt+a` arrives as the
        // glyph, not as alt+a.
        let cap = capture(&[("alt+a", "baseline", &[0xc3, 0xa5])]);
        let expected = Decoded::Key {
            input: KeyInput::normalized(Key::Char('å'), Modifiers::NONE),
            consumed: 2,
        };
        assert_eq!(
            verdict(&cap, "alt+a"),
            Some(Reachability::Unreachable { decoded: expected })
        );
    }

    #[test]
    fn an_eaten_press_is_unreachable_with_incomplete_evidence() {
        // Empty bytes = the OS/mux swallowed the press (e.g. cmd+1, ctrl+a in
        // tmux). It is unreachable, and the evidence is Incomplete — distinct
        // from "no evidence", which is absence from the result entirely.
        let cap = capture(&[("cmd+1", "baseline", &[])]);
        assert_eq!(
            verdict(&cap, "cmd+1"),
            Some(Reachability::Unreachable {
                decoded: Decoded::Incomplete
            })
        );
    }

    #[test]
    fn the_reachability_law_holds_in_both_directions() {
        // Same intended chord, two byte forms -> opposite verdicts (proves
        // Reachable implies decode==intended): `alt+a` as Meta vs as composed å.
        let meta_camp = capture(&[("alt+a", "baseline", &[0x1b, 0x61])]);
        let glyph_camp = capture(&[("alt+a", "baseline", &[0xc3, 0xa5])]);
        assert_eq!(verdict(&meta_camp, "alt+a"), Some(Reachability::Reachable));
        assert!(matches!(
            verdict(&glyph_camp, "alt+a"),
            Some(Reachability::Unreachable { .. })
        ));

        // Same bytes, two different intended chords -> opposite verdicts (proves
        // decode==intended implies Reachable, i.e. it compares, not pattern-matches
        // the bytes): c3a5 is reachable-as-`å` but unreachable-as-`alt+a`.
        let bytes: &[u8] = &[0xc3, 0xa5];
        let as_glyph = capture(&[("å", "baseline", bytes)]);
        let as_alt = capture(&[("alt+a", "baseline", bytes)]);
        assert_eq!(verdict(&as_glyph, "å"), Some(Reachability::Reachable));
        assert!(matches!(
            verdict(&as_alt, "alt+a"),
            Some(Reachability::Unreachable { .. })
        ));
    }

    #[test]
    fn an_unrecorded_chord_has_no_entry_not_a_reachable_verdict() {
        // The capture knows only ctrl+a; asking after `esc` yields no entry,
        // which must NOT be confused with Reachable.
        let cap = capture(&[("ctrl+a", "baseline", &[0x01])]);
        assert_eq!(verdict(&cap, "esc"), None);
        assert_eq!(reachability(&cap).len(), 1);
    }

    #[test]
    fn unusable_rows_are_skipped() {
        let cap = capture(&[
            ("ctrl+a", "baseline", &[0x01]),          // good
            ("not a key", "baseline", &[0x01]),       // unparseable intended
            ("ctrl+i", "kitty-enhanced-v9", &[0x09]), // unknown mode
        ]);
        // Only the good row contributes; the two unusable rows are dropped.
        assert_eq!(reachability(&cap).len(), 1);
        assert_eq!(verdict(&cap, "ctrl+a"), Some(Reachability::Reachable));
    }

    #[test]
    fn result_is_sorted_by_canonical_chord() {
        let cap = capture(&[
            ("esc", "baseline", &[0x1b]),
            ("ctrl+a", "baseline", &[0x01]),
        ]);
        let chords: Vec<String> = reachability(&cap)
            .iter()
            .map(|(k, _)| k.to_string())
            .collect();
        assert_eq!(chords, vec!["ctrl+a".to_string(), "esc".to_string()]);
    }
}
