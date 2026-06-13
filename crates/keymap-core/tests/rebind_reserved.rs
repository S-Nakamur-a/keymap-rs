//! Reserved-key (escape-hatch) survival under runtime rebinding.
//!
//! Runtime rebinding needs no new library type — it is composition (see
//! `docs/ROADMAP.md`). But the *discipline* that keeps it safe — never let a
//! rebind steal the user's way out — is security-critical, so it is pinned here
//! against the public primitives (`resolve_layered`, `bind`, `legacy_form`,
//! `Clone`) rather than left only in the `keymap-config` `rebind` example, which
//! CI compiles but does not run. If `resolve_layered`'s composition or
//! `legacy_form`'s collapse rule ever regressed, these break.
//!
//! The check has two arms, matching the example's `validate_rebind`:
//! - **legacy collapse**: a chord that sends the same C0 byte as a reserved chord
//!   would also fire on it (`ctrl+i` ≡ `tab`) — refused on the structural fact.
//! - **resolution survival**: simulate the rebind in a clone, then re-resolve
//!   every reserved key against the *composed* layer stack; refuse if any changes.

use keymap_core::{
    BreakReason, Key, KeyInput, Keymap, LegacyForm, Modifiers, RebindVerdict, resolve_layered,
    validate_rebind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Save,
    Quit,
}

fn plain(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::NONE)
}

fn ctrl(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::CTRL)
}

fn esc() -> KeyInput {
    KeyInput::new(Key::Esc, Modifiers::NONE)
}

/// The pure check a safe rebind UI runs *before* mutating: does binding
/// `proposed -> action` into layer `target` break any reserved key? Mirrors the
/// `rebind` example's `validate_rebind`, condensed to the refusal decision.
fn would_break_escape<A: Clone + PartialEq>(
    layers: &[&Keymap<A>],
    target: usize,
    proposed: KeyInput,
    action: A,
    reserved: &[KeyInput],
) -> bool {
    if let LegacyForm::CollapsesTo(byte_twin) = proposed.legacy_form() {
        if reserved.contains(&byte_twin) {
            return true;
        }
    }
    let mut simulated: Vec<Keymap<A>> = layers.iter().map(|l| (*l).clone()).collect();
    simulated[target].bind(proposed, action);
    reserved.iter().any(|&r| {
        resolve_layered(layers.iter().copied(), &r) != resolve_layered(simulated.iter(), &r)
    })
}

#[test]
fn direct_theft_of_a_reserved_key_is_refused() {
    // `esc` is the (out-of-band) escape hatch, bound to nothing. Rebinding it onto
    // an action steals it: None -> Some, so resolution changes.
    let global: Keymap<Action> = Keymap::new();
    let reserved = [esc()];
    assert!(would_break_escape(
        &[&global],
        0,
        esc(),
        Action::Save,
        &reserved
    ));
}

#[test]
fn an_upper_layer_shadowing_a_reserved_key_is_refused() {
    // The case a layer-0-only check misses: `esc` is fine in the base, but
    // rebinding it into the overlay shadows it for the whole composed stack.
    let mut base = Keymap::new();
    base.bind(esc(), Action::Quit); // escape resolves to Quit via the base
    let overlay: Keymap<Action> = Keymap::new();
    let reserved = [esc()];
    // Layers as resolve_layered sees them: overlay first, then base.
    assert!(would_break_escape(
        &[&overlay, &base],
        0, // rebind into the overlay
        esc(),
        Action::Save,
        &reserved
    ));
}

#[test]
fn rebinding_a_reserved_key_in_a_shadowed_lower_layer_is_allowed() {
    // The nuance the composed re-resolution gets right: escape lives in the
    // overlay (upper), so rebinding `esc` in the base (lower) is harmless — the
    // overlay still wins, resolution is unchanged.
    let mut overlay = Keymap::new();
    overlay.bind(esc(), Action::Quit);
    let base: Keymap<Action> = Keymap::new();
    let reserved = [esc()];
    assert!(!would_break_escape(
        &[&overlay, &base],
        1, // rebind into the base (lower) layer
        esc(),
        Action::Save,
        &reserved
    ));
}

#[test]
fn rebinding_a_reserved_key_to_its_own_current_action_is_allowed() {
    // Redundant, not destructive: resolution is unchanged, so it is not refused.
    let mut global = Keymap::new();
    global.bind(esc(), Action::Quit);
    let reserved = [esc()];
    assert!(!would_break_escape(
        &[&global],
        0,
        esc(),
        Action::Quit, // same action it already has
        &reserved
    ));
}

#[test]
fn a_legacy_collapse_onto_a_reserved_key_is_refused() {
    // `ctrl+i` is a distinct chord, but on a baseline terminal it is byte 0x09 —
    // the same as `tab`. If `tab` is reserved, binding `ctrl+i` steals it there.
    let global: Keymap<Action> = Keymap::new();
    let reserved = [KeyInput::new(Key::Tab, Modifiers::NONE)];
    assert!(would_break_escape(
        &[&global],
        0,
        ctrl('i'),
        Action::Save,
        &reserved
    ));
}

#[test]
fn an_ordinary_rebind_is_allowed() {
    // `ctrl+s` is neither reserved nor a collapse onto one — allowed even though
    // it overrides an existing ordinary binding (shadowing a non-reserved action
    // is the user's call, surfaced as an advisory elsewhere, not a refusal).
    let mut global = Keymap::new();
    global.bind(ctrl('s'), Action::Quit);
    let reserved = [esc()];
    assert!(!would_break_escape(
        &[&global],
        0,
        ctrl('s'),
        Action::Save,
        &reserved
    ));
}

#[test]
fn validation_does_not_mutate_the_live_keymap() {
    // Check-before-mutate: a refused rebind must leave the real map untouched (the
    // in-memory lockout hole — validating only at save time would already have
    // killed escape this session). The check clones, so the original is intact.
    let global: Keymap<Action> = Keymap::new();
    let reserved = [esc()];
    assert!(would_break_escape(
        &[&global],
        0,
        esc(),
        Action::Save,
        &reserved
    ));
    // The reserved key is still unbound in the live map.
    assert_eq!(global.get(&esc()), None);
    assert!(global.is_empty());
}

/// `validate_rebind` (virtual overlay, no clone) must agree with `would_break_escape`
/// (clone-simulate) for every **distinct-action** case in a representative matrix.
///
/// The one intentional divergence — re-binding a reserved key to its *same* action —
/// is excluded here and pinned separately below: `would_break_escape` permits it
/// (resolution unchanged), while `validate_rebind` refuses it (strict, no A:PartialEq
/// bound). Mixing it in would produce a false equivalence failure.
#[test]
fn validate_rebind_and_clone_simulate_agree_on_distinct_action_matrix() {
    // Matrix axes:
    //   proposed    : ctrl+s (ordinary), esc (reserved direct steal), ctrl+i (legacy collapse to tab)
    //   layers      : single-layer empty, single-layer with esc bound, two-layer overlay+base
    //   reserved set: [esc], [esc, tab]
    //
    // For each combination, the distinct action is always Action::Save (never the
    // existing action on the reserved key itself).

    let proposed_ordinary = ctrl('s');
    let proposed_reserved = esc();
    let proposed_collapse = KeyInput::new(Key::Char('i'), Modifiers::CTRL); // ctrl+i -> tab

    let reserved_esc: &[KeyInput] = &[esc()];
    let reserved_esc_tab: &[KeyInput] = &[esc(), KeyInput::new(Key::Tab, Modifiers::NONE)];

    // Helper: run both methods and assert they agree. distinct_action must differ
    // from any action already on the reserved key, so the same-action divergence
    // path is never hit.
    let assert_agree = |layers: &[&Keymap<Action>],
                        target: usize,
                        proposed: KeyInput,
                        reserved: &[KeyInput]| {
        let clone_result = would_break_escape(layers, target, proposed, Action::Save, reserved);
        let vr = validate_rebind(layers, target, proposed, reserved);
        let vr_blocks = matches!(
            vr,
            RebindVerdict::BreaksReserved {
                reason: BreakReason::DirectSteal,
                ..
            } | RebindVerdict::BreaksReserved {
                reason: BreakReason::LegacyCollapse,
                ..
            }
        );
        assert_eq!(
            clone_result, vr_blocks,
            "disagreement: proposed={proposed}, reserved={reserved:?}, clone={clone_result}, validate={vr_blocks}"
        );
    };

    // ── single empty layer ──
    let empty: Keymap<Action> = Keymap::new();

    assert_agree(&[&empty], 0, proposed_ordinary, reserved_esc);
    assert_agree(&[&empty], 0, proposed_reserved, reserved_esc);
    assert_agree(&[&empty], 0, proposed_collapse, reserved_esc_tab);

    // ── single layer with esc bound to Quit ──
    let mut with_esc = Keymap::new();
    with_esc.bind(esc(), Action::Quit);

    assert_agree(&[&with_esc], 0, proposed_ordinary, reserved_esc);
    // proposed_reserved with distinct action Save != Quit: clone sees resolution change,
    // validate_rebind refuses both. They must agree.
    assert_agree(&[&with_esc], 0, proposed_reserved, reserved_esc);
    assert_agree(&[&with_esc], 0, proposed_collapse, reserved_esc_tab);

    // ── two-layer: overlay empty, base has esc bound ──
    let overlay: Keymap<Action> = Keymap::new();
    let mut base = Keymap::new();
    base.bind(esc(), Action::Quit);

    // Rebind into overlay (upper): steals esc resolution.
    assert_agree(&[&overlay, &base], 0, proposed_ordinary, reserved_esc);
    assert_agree(&[&overlay, &base], 0, proposed_reserved, reserved_esc);
    assert_agree(&[&overlay, &base], 0, proposed_collapse, reserved_esc_tab);

    // Rebind into base (lower): esc is already in overlay, so base rebind is
    // shielded — both methods should report NOT blocked for proposed_reserved here
    // only when overlay has esc bound. Adjust: overlay has esc, base doesn't.
    let mut overlay2 = Keymap::new();
    overlay2.bind(esc(), Action::Quit);
    let base2: Keymap<Action> = Keymap::new();

    assert_agree(&[&overlay2, &base2], 1, proposed_ordinary, reserved_esc);
    assert_agree(&[&overlay2, &base2], 1, proposed_reserved, reserved_esc);
    assert_agree(&[&overlay2, &base2], 1, proposed_collapse, reserved_esc_tab);
}

/// The intentional divergence (same-action re-bind onto reserved):
/// `would_break_escape` permits it (resolution unchanged); `validate_rebind` refuses it.
/// This test pins that difference as an explicit specification fact.
#[test]
fn validate_rebind_and_clone_simulate_diverge_on_same_action_rebind() {
    let mut global: Keymap<Action> = Keymap::new();
    global.bind(esc(), Action::Quit);
    let reserved = [esc()];

    // Clone-simulate: resolution is unchanged (Quit -> Quit), so it is allowed.
    let clone_result = would_break_escape(&[&global], 0, esc(), Action::Quit, &reserved);
    assert!(!clone_result, "clone-simulate permits same-action rebind");

    // validate_rebind: strict, refuses regardless of existing action.
    let vr = validate_rebind(&[&global], 0, esc(), &reserved);
    assert!(
        matches!(vr, RebindVerdict::BreaksReserved { .. }),
        "validate_rebind refuses same-action rebind onto reserved"
    );
}

#[test]
fn multiple_reserved_keys_are_all_protected() {
    // Any one broken reserved key refuses the whole rebind.
    let global: Keymap<Action> = Keymap::new();
    let reserved = [esc(), ctrl('c'), plain('q')];
    // Stealing the third reserved key is caught just like the first.
    assert!(would_break_escape(
        &[&global],
        0,
        plain('q'),
        Action::Save,
        &reserved
    ));
}
