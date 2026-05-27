//! Interactive runtime rebinding, wired from existing primitives.
//!
//! "Open a list, pick an action, press the new key, persist it" needs **no new
//! library type** — it is composition (see `docs/ROADMAP.md`). This example is
//! the wiring, and it is deliberately a *security exemplar*: a careless rebind UI
//! is how a user ends up unable to quit their own editor. The flow is:
//!
//! 1. **Capture** the next key by decoding the raw bytes the terminal sent
//!    (`keymap_term::decode`). This is where `keymap-rs`'s terminal-awareness pays
//!    off for the *end user*: `alt+a` may arrive as `å`, `ctrl+i` as `Tab`, and a
//!    key the OS ate never arrives at all — surfaced here, not discovered later.
//! 2. **Validate** the proposed chord *before* mutating anything: never let a
//!    rebind steal the escape hatch. The check runs on the **composed** layer
//!    resolution, so an upper layer cannot shadow a reserved key in a lower one.
//! 3. **Bind** only if allowed, then **serialize** with `keymap_config::to_toml`
//!    for the caller to persist.
//!
//! State, the terminal read loop, the idle timeout, and the actual file write
//! stay caller-side — the library holds no clock and does no I/O. Run with
//! `cargo run -p keymap-config --example rebind`.

use keymap_config::to_toml;
use keymap_core::{Key, KeyInput, Keymap, LegacyForm, Modifiers, resolve_layered};
use keymap_seq::SequenceKeymap;
use keymap_term::{DecodeMode, Decoded, decode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    CursorDown,
    Save,
    Split,
}

impl Action {
    /// The config name, mirroring a `from_str` resolver (the `name_of` `to_toml`
    /// wants). A total mapping here means `to_toml` exports every binding.
    fn name(self) -> &'static str {
        match self {
            Action::CursorDown => "cursor_down",
            Action::Save => "save",
            Action::Split => "split",
        }
    }
}

fn main() {
    // The app's escape hatch: keys the user must never be able to rebind away,
    // whatever the config says. This set is the caller's policy, passed in — the
    // library never names a "reserved" concept (same reason the action type is
    // yours). Here `esc` and `ctrl+c` are handled out-of-band, so they resolve to
    // nothing in the layers; the validator still protects them.
    let reserved = [
        KeyInput::new(Key::Esc, Modifiers::NONE),
        KeyInput::new(Key::Char('c'), Modifiers::CTRL),
    ];

    // The starting keymap (one global layer). `j` already moves the cursor.
    let mut global = Keymap::new();
    global.bind(
        KeyInput::new(Key::Char('j'), Modifiers::NONE),
        Action::CursorDown,
    );
    global.bind(
        KeyInput::new(Key::Char('x'), Modifiers::CTRL),
        Action::Split,
    );

    println!("== capture: what the terminal actually delivers ==");
    capture_demo();

    println!("\n== validate before mutate: the escape hatch must survive ==");
    // A single-layer rebind of Save onto ctrl+s: the user pressed it, it decodes
    // cleanly, and it is neither reserved nor a collapse risk — allowed.
    let cs = capture(&[0x13], DecodeMode::Baseline).expect("ctrl+s decodes to a key");
    let layers = [&global];
    report(
        "rebind Save ->",
        cs,
        validate_rebind(&layers, 0, cs, Action::Save, &reserved),
    );

    // The user fat-fingers Escape while trying to pick a key: refused, and the
    // map is *not* touched (we only `bind` on Allowed, below).
    let esc = capture(&[0x1b], DecodeMode::Baseline).expect("esc decodes to a key");
    report(
        "rebind Save ->",
        esc,
        validate_rebind(&layers, 0, esc, Action::Save, &reserved),
    );

    // Rebinding onto `j` is allowed but overrides CursorDown — an advisory, not a
    // refusal: shadowing an ordinary action is the user's call; shadowing the
    // escape hatch is not.
    let j = capture(&[0x6a], DecodeMode::Baseline).expect("'j' decodes to a key");
    report(
        "rebind Split ->",
        j,
        validate_rebind(&layers, 0, j, Action::Split, &reserved),
    );

    println!("\n== composed resolution: an upper layer cannot shadow reserved ==");
    // Two active layers, overlay first (as `resolve_layered` sees them). Rebinding
    // `esc` into the *overlay* would shadow the reserved key for the whole app —
    // caught only because we re-resolve the composed stack, not just layer 0.
    let overlay: Keymap<Action> = Keymap::new();
    let two = [&overlay, &global];
    report(
        "rebind Save into overlay ->",
        esc,
        validate_rebind(&two, 0, esc, Action::Save, &reserved),
    );

    println!("\n== legacy collapse: ctrl+i ≡ tab on a baseline terminal ==");
    // A kitty-enhanced terminal can deliver a *real* ctrl+i (distinct from Tab).
    // But if `tab` is reserved, binding ctrl+i still steals it on legacy
    // terminals, where both are byte 0x09 — refused on the structural fact alone.
    let tab_reserved = [KeyInput::new(Key::Tab, Modifiers::NONE)];
    let ctrl_i = KeyInput::new(Key::Char('i'), Modifiers::CTRL);
    report(
        "rebind Save ->",
        ctrl_i,
        validate_rebind(&layers, 0, ctrl_i, Action::Save, &tab_reserved),
    );

    println!("\n== persist: serialize for the caller to write ==");
    // Commit the one rebind that was allowed, then serialize. We bind only after a
    // successful validate — never mutate then check.
    if let RebindVerdict::Allowed { .. } = validate_rebind(&layers, 0, cs, Action::Save, &reserved)
    {
        global.bind(cs, Action::Save);
    }
    let toml = to_toml(&global, &SequenceKeymap::<Action>::new(), |a| {
        Some(a.name())
    });
    print!("{toml}");
    println!(
        "// Write this to the user's *config directory* (a trusted path), where the\n\
         // reserved guard above is advisory. Pointing a rebind at a PTY-writable\n\
         // per-project file instead crosses a trust boundary: there the guard must\n\
         // be a hard error (see docs/ROADMAP.md). Round-trip is semantic, not\n\
         // byte-exact — comments, ordering, and `shift+a`-style spellings are lost."
    );
}

/// Decodes the first complete key press at the front of `bytes`, returning it
/// only when the bytes form an actual key.
///
/// This is the capture gate (ROADMAP firing-point ④): an `Incomplete` prefix
/// means "read more", and `Unrecognized` bytes are not a key — **neither is
/// promoted to a binding**. Because the return type is `Option<KeyInput>`, a
/// non-`Key` decode structurally cannot reach `bind`. The `_` arm keeps that true
/// if `Decoded` grows a future variant (it is `#[non_exhaustive]`).
fn capture(bytes: &[u8], mode: DecodeMode) -> Option<KeyInput> {
    match decode(bytes, mode) {
        Decoded::Key { input, .. } => Some(input),
        // `Incomplete` (read more) and `Unrecognized` (not a key) are both "no
        // binding candidate", and so is any future `#[non_exhaustive]` variant —
        // nothing but a real `Key` is ever promoted toward `bind`.
        _ => None,
    }
}

/// Shows that capture reflects the wire, not the intent — the end-user-facing
/// payoff of measurement-first decoding.
fn capture_demo() {
    // alt+a on an Option-composes terminal arrives as the glyph `å`, no modifier.
    show("alt+a (bytes c3 a5)", &[0xc3, 0xa5]);
    // ctrl+i and Tab are the same C0 byte on a baseline terminal.
    show("ctrl+i (byte 09)", &[0x09]);
    // A truncated escape sequence is not yet a key: keep reading.
    show("truncated CSI (bytes 1b 5b)", &[0x1b, 0x5b]);
    // A stray continuation byte is not a key at all.
    show("invalid byte (ff)", &[0xff]);
}

fn show(label: &str, bytes: &[u8]) {
    match capture(bytes, DecodeMode::Baseline) {
        Some(input) => println!("  {label} -> captured {input}"),
        None => match decode(bytes, DecodeMode::Baseline) {
            Decoded::Incomplete => println!("  {label} -> incomplete, keep reading"),
            _ => println!("  {label} -> not a key, rejected"),
        },
    }
}

/// What validating a proposed rebind concluded.
#[derive(Clone, Copy)]
enum RebindVerdict<'a> {
    /// Refused: the rebind would break the escape hatch. Carries the reserved key
    /// it would steal and why.
    BreaksEscape {
        reserved: KeyInput,
        why: &'static str,
    },
    /// Allowed. `shadows` is what the chord currently resolves to (you would be
    /// overriding it), and `legacy` flags whether it survives a legacy terminal.
    Allowed {
        shadows: Option<&'a Action>,
        legacy: LegacyForm,
    },
}

/// Validates a proposed rebind of `action` onto `proposed` in layer `target`,
/// **without mutating** the live keymap.
///
/// The escape hatch is protected two ways:
/// - *Legacy collapse*: if `proposed` sends the same byte as a reserved chord on
///   a legacy terminal (`ctrl+i` ≡ `tab`), binding it would also fire on that
///   reserved key — refused on the structural fact, no terminal measurement.
/// - *Resolution survival*: simulate the rebind in a clone and re-resolve every
///   reserved key against the **composed** layer stack. If any now resolves to
///   something different (stolen directly, or shadowed from an upper layer), the
///   rebind is refused. This is why the whole layer list is taken, not one layer.
fn validate_rebind<'a>(
    layers: &[&'a Keymap<Action>],
    target: usize,
    proposed: KeyInput,
    action: Action,
    reserved: &[KeyInput],
) -> RebindVerdict<'a> {
    if let LegacyForm::CollapsesTo(byte_twin) = proposed.legacy_form() {
        if reserved.contains(&byte_twin) {
            return RebindVerdict::BreaksEscape {
                reserved: byte_twin,
                why: "collapses onto a reserved key on legacy terminals",
            };
        }
    }

    let mut simulated: Vec<Keymap<Action>> = layers.iter().map(|l| (*l).clone()).collect();
    simulated[target].bind(proposed, action);
    for &r in reserved {
        let before = resolve_layered(layers.iter().copied(), &r);
        let after = resolve_layered(simulated.iter(), &r);
        if before != after {
            return RebindVerdict::BreaksEscape {
                reserved: r,
                why: "would steal or shadow this reserved key",
            };
        }
    }

    RebindVerdict::Allowed {
        shadows: resolve_layered(layers.iter().copied(), &proposed),
        legacy: proposed.legacy_form(),
    }
}

fn report(what: &str, proposed: KeyInput, verdict: RebindVerdict<'_>) {
    match verdict {
        RebindVerdict::BreaksEscape { reserved, why } => {
            println!("  {what} {proposed}: REFUSED — {why} ({reserved})");
        }
        RebindVerdict::Allowed { shadows, legacy } => {
            let mut notes = Vec::new();
            if let Some(prev) = shadows {
                notes.push(format!("overrides {}", prev.name()));
            }
            match legacy {
                LegacyForm::Representable => {}
                LegacyForm::CollapsesTo(t) => notes.push(format!("legacy: indistinct from {t}")),
                LegacyForm::Unrepresentable => {
                    notes.push("legacy: not deliverable on a C0 terminal".to_string());
                }
            }
            let suffix = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join("; "))
            };
            println!("  {what} {proposed}: allowed{suffix}");
        }
    }
}
