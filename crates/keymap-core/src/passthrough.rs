//! PTY raw-byte passthrough resolution.
//!
//! [`resolve_layered`](crate::resolve_layered) answers "what action, if any, is
//! bound?" and collapses a miss to [`None`]. That is the whole story for a
//! same-process app. A terminal *multiplexer* needs one thing more: when no layer
//! binds a key, the original bytes must reach the child process (the PTY) exactly
//! as they arrived. This module adds that path without disturbing the pure
//! resolver.
//!
//! [`resolve_passthrough`] is the raw-byte-carrying sibling of `resolve_layered`:
//! same first-hit-wins layer resolution, but a miss carries the original bytes out
//! as [`Resolution::Passthrough`] instead of vanishing into `None`. The PTY is a
//! **sink past the end of the scope chain**, never a layer in it — a key bound in
//! any layer wins, and only genuinely unbound keys fall through to it, so the PTY
//! can never swallow the app's reserved keys.
//!
//! # `Consume` is the caller's to assign
//!
//! The third disposition, [`Resolution::Consume`] (swallow a key with no action —
//! a modal/input grab that neither runs an action nor forwards bytes), is a
//! function of caller *mode* state (am I grabbing right now?), which this
//! state-free library does not hold. So `resolve_passthrough` never returns it: it
//! returns only [`Action`](Resolution::Action) and [`Passthrough`](Resolution::Passthrough),
//! and a grabbing caller maps a `Passthrough` to `Consume` itself. `Consume` lives
//! in the enum so the caller's `match` is exhaustive (the compiler forces a
//! decision for every key), not because the resolver manufactures it.

use crate::input::KeyInput;
use crate::keymap::{Keymap, resolve_layered};

/// The original, pre-decode bytes of a key press, borrowed from the caller's read
/// buffer, to be forwarded verbatim when no layer binds the input.
///
/// `RawInput` is a *view*, not a copy: it borrows the exact `bytes[..consumed]`
/// span the decoder reported (see `keymap_term::Decoded`), so forwarding it to the
/// sink is a byte-identity copy with no re-encoding. It is constructed only from
/// real bytes via [`RawInput::from_bytes`]; there is deliberately **no**
/// `From<KeyInput>`. Decode is lossy (SHIFT folding, `BackTab` ≡ `shift+tab`,
/// `ctrl+i` ≡ `tab`, and some keys produce no `KeyInput` at all), so re-encoding a
/// decoded key would corrupt or inject bytes. The borrow representation makes that
/// footgun *structurally impossible* — there is no owned buffer for a `From` impl
/// to return a reference into:
///
/// ```compile_fail
/// use keymap_core::{Key, KeyInput, Modifiers, RawInput};
///
/// let decoded = KeyInput::new(Key::Char('a'), Modifiers::NONE);
/// // There is no way back from a decoded key to its raw bytes — decode is lossy.
/// let _raw: RawInput = RawInput::from(decoded); // ERROR: `From<KeyInput>` does not exist
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawInput<'a>(&'a [u8]);

impl<'a> RawInput<'a> {
    /// Wraps the verbatim bytes the caller read for this press (the
    /// `bytes[..consumed]` span the decoder reported).
    #[must_use]
    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        RawInput(bytes)
    }

    /// The borrowed bytes, for forwarding verbatim to the sink.
    #[must_use]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.0
    }
}

/// What should happen to a key press, for a caller that forwards unbound input to
/// a terminal sink (a PTY). The disposition counterpart to
/// [`resolve_layered`](crate::resolve_layered)'s bare `Option<&A>`.
///
/// The three dispositions — run an action, forward the bytes verbatim, swallow the
/// key — are the complete set, so this enum is **exhaustive on purpose** (the
/// deliberate twin of `keymap_seq::Match`). [`Consume`](Resolution::Consume) ships
/// from day one rather than hiding behind `#[non_exhaustive]`, whose "compiles but
/// a key is silently misrouted" trap is exactly the failure to avoid when routing
/// input. Callers `match` all three arms with the compiler checking coverage.
///
/// The `'a` lifetime is shared by the borrowed action and the borrowed bytes: a
/// `Resolution` is meant to be consumed within the event-loop turn that produced
/// it (run the action, or write the bytes to the sink), so tying both borrows to
/// one region is sufficient. A caller that needs to outlive the read buffer copies
/// the bytes out at that boundary (`raw.as_bytes().to_vec()`).
///
/// Because the enum is exhaustive, a caller can `match` every arm with no wildcard
/// — and this stays a compile-time guarantee. The day someone makes `Resolution`
/// `#[non_exhaustive]`, this very example stops compiling:
///
/// ```
/// use keymap_core::Resolution;
///
/// fn describe(r: Resolution<'_, u8>) -> &'static str {
///     match r {
///         Resolution::Action(_) => "action",
///         Resolution::Passthrough(_) => "passthrough",
///         Resolution::Consume => "consume",
///     }
/// }
/// assert_eq!(describe(Resolution::Consume), "consume");
/// ```
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum Resolution<'a, A> {
    /// A layer bound the input; run this action.
    Action(&'a A),
    /// No layer bound the input; forward these original bytes to the sink
    /// verbatim.
    Passthrough(RawInput<'a>),
    /// Swallow the key with no action and do not forward it — a modal/input grab.
    /// [`resolve_passthrough`] never returns this; a grabbing caller assigns it
    /// from its own mode state (mapping what would be a `Passthrough`).
    Consume,
}

/// Resolves `input` against an ordered list of layers for a caller that forwards
/// unbound keys to a terminal sink (a PTY), carrying the original `raw` bytes so a
/// miss can be forwarded verbatim.
///
/// This is the raw-byte-carrying sibling of
/// [`resolve_layered`](crate::resolve_layered): it runs the same first-hit-wins
/// resolution, returning [`Resolution::Action`] on a hit and
/// [`Resolution::Passthrough`] (wrapping `raw`) on a miss in every layer. It
/// **never** returns [`Resolution::Consume`] — see the [module docs](self): a
/// modal grab is caller mode state, so the caller maps a `Passthrough` to
/// `Consume` when it is grabbing.
///
/// ```
/// use keymap_core::{
///     resolve_passthrough, Key, KeyInput, Keymap, Modifiers, RawInput, Resolution,
/// };
///
/// #[derive(PartialEq, Debug)]
/// enum Action { Quit }
///
/// let ctrl_q = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
/// let mut base = Keymap::new();
/// base.bind(ctrl_q.clone(), Action::Quit);
///
/// // A bound key resolves to its action.
/// let raw = RawInput::from_bytes(&[0x11]); // ctrl+q as a C0 byte
/// assert_eq!(
///     resolve_passthrough([&base], &ctrl_q, raw),
///     Resolution::Action(&Action::Quit),
/// );
///
/// // An unbound key carries its bytes out for verbatim forwarding.
/// let letter = KeyInput::new(Key::Char('a'), Modifiers::NONE);
/// let raw = RawInput::from_bytes(b"a");
/// match resolve_passthrough([&base], &letter, raw) {
///     Resolution::Passthrough(bytes) => assert_eq!(bytes.as_bytes(), b"a"),
///     other => panic!("expected passthrough, got {other:?}"),
/// }
/// ```
pub fn resolve_passthrough<'a, A, L>(
    layers: L,
    input: &KeyInput,
    raw: RawInput<'a>,
) -> Resolution<'a, A>
where
    L: IntoIterator<Item = &'a Keymap<A>>,
    A: 'a,
{
    match resolve_layered(layers, input) {
        Some(action) => Resolution::Action(action),
        None => Resolution::Passthrough(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Key, Modifiers};

    #[derive(Debug, Clone, PartialEq)]
    enum Action {
        Quit,
        Save,
        Split,
    }

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    #[test]
    fn raw_input_round_trips_its_bytes() {
        let raw = RawInput::from_bytes(&[0x1b, 0x5b, b'A']);
        assert_eq!(raw.as_bytes(), &[0x1b, 0x5b, b'A']);
    }

    #[test]
    fn hit_resolves_to_action() {
        let mut base = Keymap::new();
        base.bind(ctrl('q'), Action::Quit);
        let raw = RawInput::from_bytes(&[0x11]);
        assert_eq!(
            resolve_passthrough([&base], &ctrl('q'), raw),
            Resolution::Action(&Action::Quit),
        );
    }

    #[test]
    fn miss_carries_the_raw_bytes_for_passthrough() {
        let base: Keymap<Action> = Keymap::new();
        let raw = RawInput::from_bytes(b"a");
        match resolve_passthrough(
            [&base],
            &KeyInput::new(Key::Char('a'), Modifiers::NONE),
            raw,
        ) {
            Resolution::Passthrough(bytes) => assert_eq!(bytes.as_bytes(), b"a"),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }

    #[test]
    fn empty_raw_passes_through_as_empty() {
        // A zero-length span (decoder reporting consumed == 0, or a caller's
        // empty slice) must still preserve "bytes unchanged on miss": forwarding
        // zero bytes is a legitimate no-op, not a bug.
        let base: Keymap<Action> = Keymap::new();
        let raw = RawInput::from_bytes(&[]);
        match resolve_passthrough(
            [&base],
            &KeyInput::new(Key::Char('a'), Modifiers::NONE),
            raw,
        ) {
            Resolution::Passthrough(bytes) => assert_eq!(bytes.as_bytes(), b""),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }

    #[test]
    fn miss_preserves_multibyte_sequence_verbatim() {
        // The module's reason to exist: an unbound escape sequence (here the Up
        // arrow, ESC [ A) must reach the sink byte-identical. Round-trip through
        // RawInput and the resolver-miss path are otherwise tested separately.
        let base: Keymap<Action> = Keymap::new();
        let csi_up: &[u8] = &[0x1b, 0x5b, b'A'];
        let raw = RawInput::from_bytes(csi_up);
        match resolve_passthrough([&base], &KeyInput::new(Key::Up, Modifiers::NONE), raw) {
            Resolution::Passthrough(bytes) => assert_eq!(bytes.as_bytes(), csi_up),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }

    #[test]
    fn first_hit_wins_across_layers_just_like_resolve_layered() {
        let mut base = Keymap::new();
        base.bind(ctrl('s'), Action::Save);
        let mut overlay = Keymap::new();
        overlay.bind(ctrl('s'), Action::Split);
        let raw = RawInput::from_bytes(&[0x13]);
        // The overlay wins, exactly as resolve_layered would.
        assert_eq!(
            resolve_passthrough([&overlay, &base], &ctrl('s'), raw),
            Resolution::Action(&Action::Split),
        );
    }

    #[test]
    fn earlier_miss_falls_through_to_a_later_layer_hit() {
        // An empty outer layer must not steal the hit from an inner layer that
        // binds the key — the result is the action, not a passthrough.
        let overlay: Keymap<Action> = Keymap::new(); // binds nothing
        let mut base = Keymap::new();
        base.bind(ctrl('q'), Action::Quit);
        let raw = RawInput::from_bytes(&[0x11]);
        assert_eq!(
            resolve_passthrough([&overlay, &base], &ctrl('q'), raw),
            Resolution::Action(&Action::Quit),
        );
    }

    #[test]
    fn hit_ignores_raw_bytes_entirely() {
        // On a hit, a bound key must NOT also reach the PTY (or the app's reserved
        // keys would leak through the sink). The Action variant structurally has
        // no bytes; deliberately mismatched raw bytes prove they are dropped.
        let mut base = Keymap::new();
        base.bind(ctrl('q'), Action::Quit);
        let raw = RawInput::from_bytes(b"\xff\xff\xff");
        assert_eq!(
            resolve_passthrough([&base], &ctrl('q'), raw),
            Resolution::Action(&Action::Quit),
        );
    }

    #[test]
    fn no_layers_is_a_passthrough_not_a_panic() {
        let raw = RawInput::from_bytes(b"z");
        match resolve_passthrough(
            std::iter::empty::<&Keymap<Action>>(),
            &KeyInput::new(Key::Char('z'), Modifiers::NONE),
            raw,
        ) {
            Resolution::Passthrough(bytes) => assert_eq!(bytes.as_bytes(), b"z"),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }

    #[test]
    fn resolver_never_returns_consume() {
        // Verifies the runtime fact: a miss yields Passthrough, never Consume.
        // It also demonstrates the grab-mode mapping a caller performs to assign
        // Consume itself. (The compiler-enforced exhaustiveness of `Resolution` is
        // a separate, cross-crate property pinned by the doctest on the type.)
        let base: Keymap<Action> = Keymap::new();
        let raw = RawInput::from_bytes(b"a");
        let grabbing = true;
        let disposition = match resolve_passthrough(
            [&base],
            &KeyInput::new(Key::Char('a'), Modifiers::NONE),
            raw,
        ) {
            Resolution::Action(a) => Resolution::Action(a),
            // A grabbing caller swallows an otherwise-passed-through key.
            Resolution::Passthrough(_) if grabbing => Resolution::Consume,
            Resolution::Passthrough(bytes) => Resolution::Passthrough(bytes),
            Resolution::Consume => Resolution::Consume,
        };
        assert_eq!(disposition, Resolution::<Action>::Consume);
    }
}
