//! How a chord fares under the legacy 7-bit terminal encoding.
//!
//! Terminals that do not implement an enhanced keyboard protocol (the kitty
//! keyboard protocol, xterm's `modifyOtherKeys`, …) encode keys with the old
//! C0 control-code scheme, which cannot carry every chord a modern keyboard can
//! produce. The classic surprises:
//!
//! - **`ctrl+shift+<letter>` is the same byte as `ctrl+<letter>`.** A control
//!   code is `letter & 0x1f` with no bit for Shift, so `ctrl+shift+s` and
//!   `ctrl+s` are *indistinguishable* — a binding to one silently fires on the
//!   other. (This is why terminal Vim/Emacs avoid `Ctrl+Shift+*` and lean on
//!   prefix/leader sequences instead — see `keymap-seq`.)
//! - **`ctrl+i` ≡ `tab`, `ctrl+m` ≡ `enter`, `ctrl+[` ≡ `esc`.** Same C0 byte.
//! - **`super`/`cmd` has no legacy encoding at all** — a legacy terminal cannot
//!   deliver it.
//!
//! [`KeyInput::legacy_form`] reports this *structural* fact from the chord
//! alone — no terminal measurement needed, so it is stable and CI-testable
//! today. It deliberately does **not** answer "will this key reach the app on
//! my terminal": Option composing a glyph (`Option+s` → `ß`), the OS eating
//! `Cmd`, or the `alt`-vs-`Esc` timing ambiguity are environment-dependent
//! *reachability* questions for the later capability-aware layer. Treat
//! `legacy_form` as the lower bound (what is lost *no matter the terminal*); the
//! reachability layer refines it with empirical per-terminal data.

use crate::{Key, KeyInput, Keymap, Modifiers};

/// How a [`KeyInput`] survives the legacy 7-bit C0 terminal encoding,
/// independent of any enhanced protocol. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyForm {
    /// Encodable and distinguishable as-is under legacy C0.
    Representable,
    /// Loses information under legacy C0 and becomes indistinguishable from this
    /// other chord (e.g. `ctrl+shift+s` → `ctrl+s`, `ctrl+i` → `tab`). A binding
    /// to the original silently behaves like the contained chord on legacy
    /// terminals — bind that chord (or use a `keymap-seq` sequence) instead.
    CollapsesTo(KeyInput),
    /// Has no legacy C0 encoding at all (any chord holding `super`/`cmd`); a
    /// legacy terminal cannot deliver it.
    Unrepresentable,
}

impl KeyInput {
    /// Reports how this chord fares under the legacy 7-bit C0 terminal encoding
    /// (see [`LegacyForm`] and the [module docs](self)).
    ///
    /// This is a pure, terminal-independent structural fact, not a reachability
    /// verdict. Only the cases that are *certain* regardless of terminal are
    /// reported as [`CollapsesTo`](LegacyForm::CollapsesTo) /
    /// [`Unrepresentable`](LegacyForm::Unrepresentable); everything else is
    /// [`Representable`](LegacyForm::Representable), which means "has a legacy C0
    /// encoding", not "guaranteed to arrive on every terminal".
    #[must_use]
    pub fn legacy_form(&self) -> LegacyForm {
        // `super`/`cmd` cannot be encoded in legacy C0 under any circumstance.
        if self.modifiers().contains(Modifiers::SUPER) {
            return LegacyForm::Unrepresentable;
        }
        let collapsed = collapse(self.key(), self.modifiers());
        if collapsed == *self {
            LegacyForm::Representable
        } else {
            LegacyForm::CollapsesTo(collapsed)
        }
    }
}

/// The chord a legacy C0 terminal actually delivers for `(key, mods)`, for the
/// cases that are certain. Returns the input unchanged when nothing is lost.
fn collapse(key: Key, mods: Modifiers) -> KeyInput {
    if mods.contains(Modifiers::CTRL) {
        if let Key::Char(c) = key {
            // `ctrl+i`/`ctrl+m`/`ctrl+[` are the same C0 byte as the named keys.
            let aliased = match c.to_ascii_lowercase() {
                'i' => Some(Key::Tab),
                'm' => Some(Key::Enter),
                '[' => Some(Key::Esc),
                _ => None,
            };
            if let Some(named) = aliased {
                // Ctrl and Shift are both absorbed into the single C0 byte; any
                // Alt (Meta / ESC-prefix) is orthogonal and survives.
                let rest = mods
                    .difference(Modifiers::CTRL)
                    .difference(Modifiers::SHIFT);
                return KeyInput::new(named, rest);
            }
            // `ctrl+shift+<letter>` sends the same byte as `ctrl+<letter>`.
            if mods.contains(Modifiers::SHIFT) {
                return KeyInput::new(key, mods.difference(Modifiers::SHIFT));
            }
        }
    }
    KeyInput::new(key, mods)
}

/// A bound chord that will not survive a legacy 7-bit (C0) terminal, reported by
/// [`legacy_lints`]. The variants mirror the two lossy [`LegacyForm`] cases (a
/// [`Representable`](LegacyForm::Representable) chord produces no lint).
///
/// Chords are rendered in canonical form (`KeyInput`'s `Display`), matching how
/// the config layer names chords in its warnings. This is `#[non_exhaustive]`:
/// the empirical capability-aware layer may add environment-dependent lint
/// categories (e.g. "may not be delivered on this terminal") additively.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegacyLint {
    /// The chord holds `super`/`cmd`: it has no legacy C0 encoding at all, so a
    /// legacy terminal can never deliver it. See
    /// [`Unrepresentable`](LegacyForm::Unrepresentable).
    Unrepresentable {
        /// The bound chord, in canonical form (e.g. `"super+s"`).
        chord: String,
    },
    /// On a legacy C0 terminal the chord is indistinguishable from another, so a
    /// binding to it also fires on `collapses_to` (e.g. `ctrl+shift+s` ≡
    /// `ctrl+s`, `ctrl+i` ≡ `tab`). See [`CollapsesTo`](LegacyForm::CollapsesTo).
    CollapsesTo {
        /// The bound chord, in canonical form.
        chord: String,
        /// The chord it is indistinguishable from on a legacy terminal, in
        /// canonical form.
        collapses_to: String,
    },
}

/// Reports every bound chord in `keymap` that will not survive a legacy 7-bit
/// (C0) terminal, derived purely from [`KeyInput::legacy_form`] — a structural
/// fact needing no terminal measurement, so this is stable and CI-testable.
///
/// This is **opt-in**: nothing computes it unless you call it, and it is wholly
/// independent of any config-layer warning surface (`keymap-config`'s
/// `BuildOutput::warnings`), so a caller gating on `warnings.is_empty()` is
/// unaffected. Pass `out.global()` (or any one layer) after a config load, or any
/// `Keymap` you built by hand.
///
/// The result is sorted by chord (canonical form); [`Keymap::iter`] is itself
/// unordered, so sorting gives stable, human-readable output. Covers
/// single-chord bindings only — multi-key sequences are a planned follow-up
/// (gated on iteration over `SequenceKeymap`).
#[must_use]
pub fn legacy_lints<A>(keymap: &Keymap<A>) -> Vec<LegacyLint> {
    let mut lints: Vec<LegacyLint> = keymap
        .iter()
        .filter_map(|(input, _)| match input.legacy_form() {
            LegacyForm::Representable => None,
            LegacyForm::Unrepresentable => Some(LegacyLint::Unrepresentable {
                chord: input.to_string(),
            }),
            LegacyForm::CollapsesTo(target) => Some(LegacyLint::CollapsesTo {
                chord: input.to_string(),
                collapses_to: target.to_string(),
            }),
        })
        .collect();
    // Each KeyInput key is unique, so the chord is a total, stable sort key.
    lints.sort_by(|a, b| lint_chord(a).cmp(lint_chord(b)));
    lints
}

/// The canonical chord string a [`LegacyLint`] is about (its sort key).
fn lint_chord(lint: &LegacyLint) -> &str {
    match lint {
        LegacyLint::Unrepresentable { chord } | LegacyLint::CollapsesTo { chord, .. } => chord,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Key, KeyInput, Modifiers};

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    fn ctrl_shift(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL | Modifiers::SHIFT)
    }

    #[test]
    fn super_chords_are_unrepresentable() {
        let cmd_s = KeyInput::new(Key::Char('s'), Modifiers::SUPER);
        assert_eq!(cmd_s.legacy_form(), LegacyForm::Unrepresentable);
        // Super dominates even when the rest would otherwise collapse.
        let cmd_shift_s = KeyInput::new(Key::Char('s'), Modifiers::SUPER | Modifiers::SHIFT);
        assert_eq!(cmd_shift_s.legacy_form(), LegacyForm::Unrepresentable);
    }

    #[test]
    fn ctrl_shift_letter_collapses_to_ctrl_letter() {
        assert_eq!(
            ctrl_shift('s').legacy_form(),
            LegacyForm::CollapsesTo(ctrl('s'))
        );
    }

    #[test]
    fn ctrl_i_m_bracket_alias_named_keys() {
        assert_eq!(
            ctrl('i').legacy_form(),
            LegacyForm::CollapsesTo(KeyInput::new(Key::Tab, Modifiers::NONE))
        );
        assert_eq!(
            ctrl('m').legacy_form(),
            LegacyForm::CollapsesTo(KeyInput::new(Key::Enter, Modifiers::NONE))
        );
        assert_eq!(
            ctrl('[').legacy_form(),
            LegacyForm::CollapsesTo(KeyInput::new(Key::Esc, Modifiers::NONE))
        );
        // ctrl+shift+i still lands on Tab (shift and ctrl both absorbed).
        assert_eq!(
            ctrl_shift('i').legacy_form(),
            LegacyForm::CollapsesTo(KeyInput::new(Key::Tab, Modifiers::NONE))
        );
    }

    #[test]
    fn plain_and_ctrl_letter_are_representable() {
        assert_eq!(
            KeyInput::new(Key::Char('a'), Modifiers::NONE).legacy_form(),
            LegacyForm::Representable
        );
        // ctrl+s (no shift) is the canonical C0 byte itself.
        assert_eq!(ctrl('s').legacy_form(), LegacyForm::Representable);
    }

    #[test]
    fn named_function_and_arrow_keys_are_representable() {
        for key in [
            Key::Tab,
            Key::Enter,
            Key::Esc,
            Key::Up,
            Key::F(1),
            Key::Home,
        ] {
            assert_eq!(
                KeyInput::new(key, Modifiers::NONE).legacy_form(),
                LegacyForm::Representable,
                "{key:?} should be representable"
            );
        }
        // shift+tab is a distinct legacy sequence (BackTab), still representable.
        assert_eq!(
            KeyInput::new(Key::Tab, Modifiers::SHIFT).legacy_form(),
            LegacyForm::Representable
        );
    }

    fn lint_map(chords: &[KeyInput]) -> Keymap<u8> {
        let mut map = Keymap::new();
        for (i, &c) in chords.iter().enumerate() {
            map.bind(c, u8::try_from(i).unwrap());
        }
        map
    }

    #[test]
    fn legacy_lints_empty_map_is_empty() {
        assert_eq!(legacy_lints(&Keymap::<u8>::new()), vec![]);
    }

    #[test]
    fn legacy_lints_classify_each_arm_with_canonical_strings() {
        // ctrl+s = Representable (no lint); cmd+s = Unrepresentable;
        // ctrl+shift+s = CollapsesTo ctrl+s; ctrl+i = CollapsesTo tab.
        let map = lint_map(&[
            KeyInput::new(Key::Char('s'), Modifiers::CTRL),
            KeyInput::new(Key::Char('s'), Modifiers::SUPER),
            KeyInput::new(Key::Char('s'), Modifiers::CTRL | Modifiers::SHIFT),
            ctrl('i'),
        ]);
        // Sorted by chord: "ctrl+i", "ctrl+shift+s", "super+s" ("ctrl+s" omitted).
        assert_eq!(
            legacy_lints(&map),
            vec![
                LegacyLint::CollapsesTo {
                    chord: "ctrl+i".to_string(),
                    collapses_to: "tab".to_string(),
                },
                LegacyLint::CollapsesTo {
                    chord: "ctrl+shift+s".to_string(),
                    collapses_to: "ctrl+s".to_string(),
                },
                LegacyLint::Unrepresentable {
                    chord: "super+s".to_string(),
                },
            ]
        );
    }

    #[test]
    fn legacy_lints_super_dominates_over_shift_collapse() {
        // cmd+shift+s holds super, so it is Unrepresentable, not a CollapsesTo —
        // the precedence must survive the lint mapping.
        let map = lint_map(&[KeyInput::new(
            Key::Char('s'),
            Modifiers::SUPER | Modifiers::SHIFT,
        )]);
        assert_eq!(
            legacy_lints(&map),
            vec![LegacyLint::Unrepresentable {
                chord: "shift+super+s".to_string(),
            }]
        );
    }

    #[test]
    fn legacy_lints_omit_representable_and_count_matches() {
        // Three representable + two not: only the two are reported (no-drop,
        // no-spurious count, independent of legacy_form's per-chord rule).
        let map = lint_map(&[
            KeyInput::new(Key::Char('a'), Modifiers::CTRL), // representable
            KeyInput::new(Key::Up, Modifiers::NONE),        // representable
            KeyInput::new(Key::F(1), Modifiers::NONE),      // representable
            KeyInput::new(Key::Char('1'), Modifiers::SUPER), // unrepresentable
            ctrl('m'),                                      // collapses to enter
        ]);
        assert_eq!(legacy_lints(&map).len(), 2);
    }

    #[test]
    fn legacy_lints_collapses_to_target_is_canonical_and_representable() {
        // Every reported collapse target must parse back and itself be
        // Representable — guards the string rendering of renamed targets.
        let map = lint_map(&[ctrl('i'), ctrl('m'), ctrl('['), ctrl_shift('s')]);
        for lint in legacy_lints(&map) {
            if let LegacyLint::CollapsesTo { collapses_to, .. } = lint {
                let parsed: KeyInput = collapses_to.parse().expect("target parses");
                assert_eq!(parsed.legacy_form(), LegacyForm::Representable);
            }
        }
    }

    #[test]
    fn collapse_target_is_itself_a_stable_legacy_form() {
        // The law: whatever a chord collapses to must itself be Representable
        // (the collapsed form is what the terminal actually delivers).
        for chord in [
            ctrl_shift('s'),
            ctrl('i'),
            ctrl('m'),
            ctrl('['),
            ctrl_shift('i'),
        ] {
            if let LegacyForm::CollapsesTo(target) = chord.legacy_form() {
                assert_eq!(
                    target.legacy_form(),
                    LegacyForm::Representable,
                    "{chord:?} collapsed to a non-stable form {target:?}"
                );
            } else {
                panic!("{chord:?} expected to collapse");
            }
        }
    }
}
