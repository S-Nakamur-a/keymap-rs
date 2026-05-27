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

use keymap_core::{Key, KeyInput, Keymap, LegacyForm, Modifiers, resolve_layered};

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
