//! Opt-in rebind validation via virtual overlay.
//!
//! [`validate_rebind`] answers "is it safe to bind `proposed` into `target`?"
//! without cloning any keymap or requiring any bounds on `A`. The full layer
//! stack is inspected once, in a single linear pass, so the check is O(R × L)
//! where R is `reserved.len()` and L is `layers.len()`.

use crate::{KeyInput, Keymap, LegacyForm};

/// Why a proposed rebind would break a reserved key.
///
/// This is `#[non_exhaustive]` because the empirical capability-aware layer
/// may add further break reasons (e.g. "would shadow on kitty protocol") in a
/// future additive release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakReason {
    /// `proposed` is identical to the reserved chord: placing it into `target`
    /// would directly steal that chord from the app's escape hatch.
    DirectSteal,
    /// On a legacy 7-bit (C0) terminal `proposed` is indistinguishable from
    /// the reserved chord (e.g. `ctrl+i` ≡ `tab`): binding `proposed` would
    /// fire on the reserved key on any terminal that does not implement an
    /// enhanced keyboard protocol.
    LegacyCollapse,
}

/// What [`validate_rebind`] concluded.
///
/// This is `#[non_exhaustive]` so that additive verdicts (e.g. an advisory
/// "allowed but unreachable on legacy terminals") can be added without
/// breaking callers. Match both arms with an explicit binding for all known
/// fields so the compiler surfaces any future field additions.
#[derive(Debug)]
#[non_exhaustive]
pub enum RebindVerdict<'a, A> {
    /// Refused. The proposed chord would break the caller's escape hatch.
    BreaksReserved {
        /// The reserved key whose resolution would be stolen.
        reserved: KeyInput,
        /// The structural reason the rebind is refused.
        reason: BreakReason,
    },
    /// Allowed. The rebind does not threaten any reserved key.
    Allowed {
        /// The action that `proposed` currently resolves to via the layer
        /// stack, if any. `Some(&a)` means the rebind would silently override
        /// that action in the target layer — worth surfacing in a UI.
        shadows: Option<&'a A>,
        /// How `proposed` fares on a legacy 7-bit C0 terminal. This is
        /// carried here so the caller can surface the legacy story alongside
        /// the safety verdict in a single call.
        legacy: LegacyForm,
    },
}

/// Validates a proposed rebind of `proposed` into layer `layers[target]`
/// **without mutating** the live keymap or requiring `A: Clone` or `A:
/// PartialEq`.
///
/// ## Semantics
///
/// The contract is strict: **a reserved chord cannot be the target of a
/// rebind, period**. A rebind of the same action onto the same chord is
/// refused just like any other rebind onto a reserved key. This is a
/// deliberate design choice: distinguishing "same action, no-op" from
/// "different action, dangerous" requires `A: PartialEq`, which this
/// function deliberately avoids; and a true no-op rebind is a UI concern
/// that should be filtered before calling this function.
///
/// If you need a more permissive variant (e.g. allowing same-action
/// rebinds), add a *sibling function* with the relaxed contract rather than
/// changing this one. Relaxing an existing function would be a silent
/// behavioural change for callers who rely on the strict semantics; adding a
/// sibling is additive.
///
/// ## Virtual overlay
///
/// No keymap is cloned. For each `reserved` key `r` the function walks the
/// layer slice once:
///
/// - If any layer *before* `target` already has a binding for `r`, that
///   layer would win regardless of what `target` contains, so adding
///   `proposed` to `target` cannot affect `r`'s resolution — the proposed
///   bind is harmless for that reserved key.
/// - Otherwise, if `proposed == r` (direct steal) or
///   `proposed.legacy_form() == CollapsesTo(r)` (legacy collapse), the
///   rebind is refused with the appropriate [`BreakReason`].
///
/// ## Panics
///
/// Panics if `target >= layers.len()`. The caller must guarantee `target`
/// is a valid index into `layers`.
///
/// ## Example
///
/// ```
/// use keymap_core::{Key, KeyInput, Keymap, Modifiers, validate_rebind,
///                   RebindVerdict, BreakReason};
///
/// let esc = KeyInput::new(Key::Esc, Modifiers::NONE);
/// let cs  = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
///
/// let global: Keymap<&str> = Keymap::new();
/// let layers = [&global];
///
/// // ctrl+s is not reserved — allowed.
/// let v = validate_rebind(&layers, 0, cs, &[esc]);
/// assert!(matches!(v, RebindVerdict::Allowed { .. }));
///
/// // Trying to bind Esc — refused.
/// let v = validate_rebind(&layers, 0, esc, &[esc]);
/// assert!(matches!(v, RebindVerdict::BreaksReserved {
///     reason: BreakReason::DirectSteal, ..
/// }));
/// ```
#[must_use]
pub fn validate_rebind<'a, A>(
    layers: &[&'a Keymap<A>],
    target: usize,
    proposed: KeyInput,
    reserved: &[KeyInput],
) -> RebindVerdict<'a, A> {
    assert!(
        target < layers.len(),
        "validate_rebind: target index {target} is out of bounds (layers.len() = {})",
        layers.len(),
    );

    // Pre-compute whether `proposed` legacy-collapses to each reserved key.
    // We compute this once and reuse it per reserved key in the loop below.
    let proposed_legacy = proposed.legacy_form();

    for &r in reserved {
        // Step 1: Is `proposed` a threat to `r` at all?
        //
        // A threat exists when `proposed` and `r` would collide in the
        // target layer:
        //   (a) Direct steal: proposed is exactly r.
        //   (b) Legacy collapse: proposed collapses to r on legacy terminals.
        let break_reason = if proposed == r {
            Some(BreakReason::DirectSteal)
        } else if let LegacyForm::CollapsesTo(twin) = proposed_legacy {
            if twin == r {
                Some(BreakReason::LegacyCollapse)
            } else {
                None
            }
        } else {
            None
        };

        let Some(reason) = break_reason else {
            // `proposed` does not collide with `r`; move on.
            continue;
        };

        // Step 2: Would `r` even reach `target`?
        //
        // Walk the layers *before* `target`. If any of them already has a
        // binding for `r`, that layer wins the resolution regardless of what
        // `target` contains — putting `proposed` into `target` cannot change
        // how `r` resolves, so this reserved key is safe.
        let already_shadowed = layers[..target].iter().any(|l| l.contains(&r));
        if already_shadowed {
            continue;
        }

        // `proposed` collides with `r` and would reach `target` — refuse.
        return RebindVerdict::BreaksReserved {
            reserved: r,
            reason,
        };
    }

    // No reserved key is threatened. Report what `proposed` currently
    // resolves to in the existing stack (the optional advisory shadow).
    let shadows = layers.iter().find_map(|l| l.get(&proposed));

    RebindVerdict::Allowed {
        shadows,
        legacy: proposed_legacy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Key, KeyInput, Keymap, Modifiers};

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    fn plain(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::NONE)
    }

    fn esc() -> KeyInput {
        KeyInput::new(Key::Esc, Modifiers::NONE)
    }

    fn tab() -> KeyInput {
        KeyInput::new(Key::Tab, Modifiers::NONE)
    }

    // ─────────────────────────────────────────────────────
    // ① Direct steal: proposed == reserved
    // ─────────────────────────────────────────────────────

    #[test]
    fn direct_steal_of_reserved_is_refused() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let v = validate_rebind(&layers, 0, esc(), &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::BreaksReserved {
                    reserved,
                    reason: BreakReason::DirectSteal
                } if reserved == esc()
            ),
            "expected DirectSteal for esc"
        );
    }

    #[test]
    fn direct_steal_is_refused_regardless_of_which_reserved_matches() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let reserved = [esc(), ctrl('c')];
        // Trying to bind ctrl+c (the second reserved key).
        let v = validate_rebind(&layers, 0, ctrl('c'), &reserved);
        assert!(
            matches!(
                v,
                RebindVerdict::BreaksReserved {
                    reason: BreakReason::DirectSteal,
                    ..
                }
            ),
            "ctrl+c is reserved, expected DirectSteal"
        );
    }

    // ─────────────────────────────────────────────────────
    // ② Upper layer already steals reserved → proposed is harmless
    // ─────────────────────────────────────────────────────

    #[test]
    fn upper_layer_already_steals_reserved_proposed_is_harmless() {
        // `esc` is reserved. The overlay (layer 0) binds esc already, so
        // putting anything onto layer 1 (global) cannot change how `esc`
        // resolves — allowed.
        let mut overlay: Keymap<&str> = Keymap::new();
        overlay.bind(esc(), "overlay_escape_handler");
        let mut global: Keymap<&str> = Keymap::new();
        global.bind(plain('j'), "cursor_down");

        let layers = [&overlay, &global];
        // Bind esc into layer 1 (global). Layer 0 (overlay) already claims it.
        let v = validate_rebind(&layers, 1, esc(), &[esc()]);
        assert!(
            matches!(v, RebindVerdict::Allowed { .. }),
            "upper layer shadows esc already; target bind is harmless"
        );
    }

    #[test]
    fn upper_layer_shadow_does_not_apply_to_target_zero() {
        // When target is 0 there are no layers before it, so no existing
        // layer can shadow `r` — the steal is always direct.
        let mut overlay: Keymap<&str> = Keymap::new();
        overlay.bind(ctrl('c'), "existing");
        let global: Keymap<&str> = Keymap::new();
        let layers = [&overlay, &global];

        // Bind ctrl+c into layer 0 (which already has it). No prior layer
        // can shadow it — refused.
        let v = validate_rebind(&layers, 0, ctrl('c'), &[ctrl('c')]);
        assert!(
            matches!(
                v,
                RebindVerdict::BreaksReserved {
                    reason: BreakReason::DirectSteal,
                    ..
                }
            ),
            "target=0 means no prior layer can shield reserved"
        );
    }

    // ─────────────────────────────────────────────────────
    // ③ Harmless bind to lower layer, with shadows advisory
    // ─────────────────────────────────────────────────────

    #[test]
    fn bind_onto_unbound_chord_is_allowed_no_shadow() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let v = validate_rebind(&layers, 0, ctrl('s'), &[esc()]);
        assert!(
            matches!(v, RebindVerdict::Allowed { shadows: None, .. }),
            "ctrl+s is not reserved and not yet bound"
        );
    }

    #[test]
    fn bind_onto_existing_chord_reports_shadow() {
        let mut global: Keymap<&str> = Keymap::new();
        global.bind(plain('j'), "cursor_down");
        let layers = [&global];
        let v = validate_rebind(&layers, 0, plain('j'), &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::Allowed {
                    shadows: Some(&"cursor_down"),
                    ..
                }
            ),
            "j has an existing binding that would be shadowed"
        );
    }

    #[test]
    fn shadow_is_read_from_any_layer_not_just_target() {
        // `ctrl+s` is bound in the inner layer (index 1), not in the overlay.
        // validate_rebind should still see it as a shadow when we bind onto
        // the overlay (index 0), because the shadow lookup walks the whole stack.
        let overlay: Keymap<&str> = Keymap::new();
        let mut global: Keymap<&str> = Keymap::new();
        global.bind(ctrl('s'), "save");
        let layers = [&overlay, &global];
        let v = validate_rebind(&layers, 0, ctrl('s'), &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::Allowed {
                    shadows: Some(&"save"),
                    ..
                }
            ),
            "shadow comes from inner layer"
        );
    }

    // ─────────────────────────────────────────────────────
    // ④ Legacy collapse: ctrl+i collapses to tab
    // ─────────────────────────────────────────────────────

    #[test]
    fn legacy_collapse_onto_reserved_is_refused() {
        // `tab` is reserved. `ctrl+i` collapses to `tab` on legacy terminals.
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let ctrl_i = KeyInput::new(Key::Char('i'), Modifiers::CTRL);
        let v = validate_rebind(&layers, 0, ctrl_i, &[tab()]);
        assert!(
            matches!(
                v,
                RebindVerdict::BreaksReserved {
                    reserved,
                    reason: BreakReason::LegacyCollapse,
                } if reserved == tab()
            ),
            "ctrl+i collapses to tab on legacy terminals — must be refused"
        );
    }

    #[test]
    fn legacy_collapse_is_harmless_when_twin_is_not_reserved() {
        // `ctrl+i` collapses to `tab`, but `tab` is not in our reserved set.
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let ctrl_i = KeyInput::new(Key::Char('i'), Modifiers::CTRL);
        let v = validate_rebind(&layers, 0, ctrl_i, &[esc()]);
        assert!(
            matches!(v, RebindVerdict::Allowed { .. }),
            "collapse target not in reserved set → allowed"
        );
    }

    #[test]
    fn legacy_collapse_shielded_by_upper_layer_is_harmless() {
        // Tab is reserved but the overlay already claims it, so binding
        // ctrl+i into the global layer cannot change tab's resolution.
        let mut overlay: Keymap<&str> = Keymap::new();
        overlay.bind(tab(), "tab_handler");
        let global: Keymap<&str> = Keymap::new();
        let layers = [&overlay, &global];
        let ctrl_i = KeyInput::new(Key::Char('i'), Modifiers::CTRL);
        let v = validate_rebind(&layers, 1, ctrl_i, &[tab()]);
        assert!(
            matches!(v, RebindVerdict::Allowed { .. }),
            "upper layer shields tab from legacy collapse in lower layer"
        );
    }

    // ─────────────────────────────────────────────────────
    // ⑤ Same-action rebind is STILL refused (strict semantics, spec-fixed)
    // ─────────────────────────────────────────────────────

    #[test]
    fn same_action_rebind_onto_reserved_is_refused() {
        // The strict contract: even a "no-op" rebind of esc → same action is
        // refused because validate_rebind has no A: PartialEq bound and
        // intentionally does not compare actions. This test pins the spec.
        let mut global: Keymap<&str> = Keymap::new();
        global.bind(esc(), "quit");
        let layers = [&global];
        let v = validate_rebind(&layers, 0, esc(), &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::BreaksReserved {
                    reason: BreakReason::DirectSteal,
                    ..
                }
            ),
            "same-action rebind onto reserved is still refused (strict contract)"
        );
    }

    // ─────────────────────────────────────────────────────
    // ⑥ target out of bounds → should_panic
    // ─────────────────────────────────────────────────────

    #[test]
    #[should_panic(expected = "target index 1 is out of bounds")]
    fn target_out_of_bounds_panics() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let _ = validate_rebind(&layers, 1, esc(), &[esc()]);
    }

    #[test]
    #[should_panic(expected = "target index 0 is out of bounds")]
    fn empty_layers_panics() {
        let layers: &[&Keymap<&str>] = &[];
        let _ = validate_rebind(layers, 0, esc(), &[]);
    }

    // ─────────────────────────────────────────────────────
    // Additional edge cases
    // ─────────────────────────────────────────────────────

    #[test]
    fn empty_reserved_set_is_always_allowed() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        let v = validate_rebind(&layers, 0, esc(), &[]);
        assert!(
            matches!(v, RebindVerdict::Allowed { .. }),
            "no reserved keys means everything is allowed"
        );
    }

    #[test]
    fn allowed_carries_correct_legacy_form_for_proposed() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        // ctrl+s is representable.
        let v = validate_rebind(&layers, 0, ctrl('s'), &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::Allowed {
                    legacy: LegacyForm::Representable,
                    ..
                }
            ),
            "ctrl+s is representable on legacy terminals"
        );
    }

    #[test]
    fn allowed_carries_collapses_to_legacy_form_when_not_reserved() {
        let global: Keymap<&str> = Keymap::new();
        let layers = [&global];
        // ctrl+shift+s collapses to ctrl+s; tab is not reserved.
        let ctrl_shift_s = KeyInput::new(Key::Char('s'), Modifiers::CTRL | Modifiers::SHIFT);
        let v = validate_rebind(&layers, 0, ctrl_shift_s, &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::Allowed {
                    legacy: LegacyForm::CollapsesTo(twin),
                    ..
                } if twin == ctrl('s')
            ),
            "ctrl+shift+s collapses to ctrl+s and should be reflected in Allowed"
        );
    }

    #[test]
    fn multi_layer_three_layers_target_middle() {
        // Three layers: [overlay(idx 0), mid(idx 1), global(idx 2)].
        // esc is reserved. Binding esc into mid (idx 1) is refused because
        // no layer before index 1 (only overlay) claims esc, so esc would
        // reach target=1.
        let overlay: Keymap<&str> = Keymap::new();
        let mid: Keymap<&str> = Keymap::new();
        let global: Keymap<&str> = Keymap::new();
        let layers = [&overlay, &mid, &global];
        let v = validate_rebind(&layers, 1, esc(), &[esc()]);
        assert!(
            matches!(
                v,
                RebindVerdict::BreaksReserved {
                    reason: BreakReason::DirectSteal,
                    ..
                }
            ),
            "no prior layer shields esc; mid target is refused"
        );
    }

    #[test]
    fn multi_layer_overlay_shields_for_target_two() {
        // Same setup, but overlay (idx 0) now binds esc.
        // Binding esc into global (idx 2) is harmless.
        let mut overlay: Keymap<&str> = Keymap::new();
        overlay.bind(esc(), "overlay_esc");
        let mid: Keymap<&str> = Keymap::new();
        let global: Keymap<&str> = Keymap::new();
        let layers = [&overlay, &mid, &global];
        let v = validate_rebind(&layers, 2, esc(), &[esc()]);
        assert!(
            matches!(v, RebindVerdict::Allowed { .. }),
            "overlay at idx 0 shields esc from target idx 2"
        );
    }
}
