//! The normalized key-input vocabulary.
//!
//! These types are backend-neutral on purpose: `crossterm`/`termion`/etc. event
//! types never appear in this module's always-compiled surface. Conversions from
//! a specific backend live in feature-gated submodules (e.g. [`crossterm`]).

#[cfg(feature = "crossterm")]
pub(crate) mod crossterm;

mod grammar;

pub use grammar::ParseKeyInputError;

/// A physical key, independent of which terminal or OS produced it.
///
/// Only keys whose identity is unambiguous across environments are enumerated
/// today. Keys whose meaning is environment-dependent (media keys, keypad
/// distinctions, etc.) are intentionally omitted and will be added later; this
/// enum is `#[non_exhaustive]`, so adding them is not a breaking change. Because
/// of that, downstream `match` expressions must include a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    /// A character-producing key, carrying the *resolved* glyph. See the
    /// normalization note on [`KeyInput`] for how shift interacts with this.
    Char(char),
    /// A function key, `F(1)` through `F(n)`.
    F(u8),
    /// The Enter / Return key.
    Enter,
    /// The Escape key.
    Esc,
    /// The Tab key. `Shift+Tab` is represented as `Tab` with [`Modifiers::SHIFT`].
    Tab,
    /// The Backspace key.
    Backspace,
    /// The forward Delete key.
    Delete,
    /// The Insert key.
    Insert,
    /// The Home key.
    Home,
    /// The End key.
    End,
    /// The Page Up key.
    PageUp,
    /// The Page Down key.
    PageDown,
    /// The Up arrow.
    Up,
    /// The Down arrow.
    Down,
    /// The Left arrow.
    Left,
    /// The Right arrow.
    Right,
}

/// A set of held modifier keys.
///
/// Backed by a private bit set so the in-memory representation can grow (e.g. to
/// `u16`) without changing the public API, and so no third-party bit-flags type
/// leaks into this crate's semver surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifiers held.
    pub const NONE: Modifiers = Modifiers(0);
    /// The Control modifier.
    pub const CTRL: Modifiers = Modifiers(0b0001);
    /// The Alt / Option modifier.
    pub const ALT: Modifiers = Modifiers(0b0010);
    /// The Shift modifier.
    pub const SHIFT: Modifiers = Modifiers(0b0100);
    /// The Super / Command / Windows modifier.
    pub const SUPER: Modifiers = Modifiers(0b1000);

    /// Returns the empty modifier set (alias for [`Modifiers::NONE`]).
    #[must_use]
    pub const fn empty() -> Self {
        Modifiers::NONE
    }

    /// Returns `true` if no modifiers are held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` if every modifier in `other` is also held in `self`.
    #[must_use]
    pub const fn contains(self, other: Modifiers) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns the union of two modifier sets.
    #[must_use]
    pub const fn union(self, other: Modifiers) -> Self {
        Modifiers(self.0 | other.0)
    }

    /// Returns `self` with every modifier in `other` removed.
    #[must_use]
    pub const fn difference(self, other: Modifiers) -> Self {
        Modifiers(self.0 & !other.0)
    }
}

impl core::ops::BitOr for Modifiers {
    type Output = Modifiers;

    fn bitor(self, rhs: Modifiers) -> Modifiers {
        self.union(rhs)
    }
}

impl core::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Modifiers) {
        self.0 |= rhs.0;
    }
}

/// A normalized key press: a [`Key`] plus the [`Modifiers`] held with it.
///
/// This is the unit a [`Keymap`](crate::Keymap) is keyed on, so its equality is
/// the equality used for binding lookup.
///
/// # Normalization
///
/// For character-producing keys, the terminal has already resolved which glyph
/// the keypress produces (`A`, `!`, `あ`, …). That resolution depends on the
/// keyboard layout, which this crate cannot reconstruct from the glyph alone, so
/// `keymap-core` does *not* re-case or re-map it.
///
/// [`Modifiers::SHIFT`] is cleared for a character key **only on a bare press**
/// (when Shift is the sole modifier), because there it is redundant with the
/// resolved glyph: `Char('A')` with Shift held normalizes to `Char('A')`, and
/// `"shift+a"` parses to the same `KeyInput` as `"a"`. When `Ctrl`, `Alt`, or
/// `Super` is *also* held, terminals send the base glyph plus modifier bits
/// (e.g. `Ctrl+Shift+s` arrives as `Char('s')` + `CTRL` + `SHIFT`), so Shift is
/// the only signal distinguishing the chord from `Ctrl+s` and is **kept**. This
/// is what makes `cmd+shift+s` usable as a binding distinct from `cmd+s`.
/// `Ctrl`, `Alt`, and `Super` are always preserved; for non-character keys
/// (e.g. `Tab`, arrows) `Shift` is kept as a modifier.
///
/// Two consequences left to a later capability-aware layer (not handled here):
/// if a terminal reports the *base-layout* key (`Char('a')` + Shift) instead of
/// the shifted glyph, a bare press normalizes to `Char('a')` and won't match a
/// `Char('A')` binding; and symbol chords (`shift+1` → `!` vs `1`+Shift) cannot
/// be reconciled across terminals without layout knowledge.
///
/// Constructing a `KeyInput` directly via [`KeyInput::new`] does not apply this
/// normalization — it is applied at backend boundaries (e.g. the `crossterm`
/// conversion). Pass already-normalized values to `new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct KeyInput {
    key: Key,
    mods: Modifiers,
}

impl KeyInput {
    /// Builds a `KeyInput` from an already-normalized key and modifier set.
    #[must_use]
    pub const fn new(key: Key, mods: Modifiers) -> Self {
        KeyInput { key, mods }
    }

    /// Builds a `KeyInput`, applying the shared [`normalize`] rule first.
    ///
    /// This is the constructor every boundary that turns *raw* key parts into a
    /// `KeyInput` should use — the string grammar, the `crossterm` adapter, and
    /// the `keymap-term` byte decoder all funnel through it. Routing them through
    /// one normalizing constructor is what guarantees a binding parsed from text
    /// and an event produced at runtime land on the *same* `KeyInput`, so lookup
    /// can't silently miss. Prefer this over [`new`](KeyInput::new) unless the
    /// parts are already known to be normalized.
    #[must_use]
    pub fn normalized(key: Key, mods: Modifiers) -> Self {
        let (key, mods) = normalize(key, mods);
        KeyInput { key, mods }
    }

    /// The key that was pressed.
    #[must_use]
    pub const fn key(&self) -> Key {
        self.key
    }

    /// The modifiers held during the press.
    #[must_use]
    pub const fn modifiers(&self) -> Modifiers {
        self.mods
    }
}

/// The single input-normalization rule, shared by every boundary that produces a
/// [`KeyInput`] (the backend conversion and the string grammar). Keeping it in
/// one place is what guarantees a binding parsed from text and an event produced
/// at runtime normalize to the *same* `KeyInput`, so lookup can't silently miss.
///
/// For character keys, [`Modifiers::SHIFT`] is cleared only when it is the sole
/// modifier (a bare press, where Shift is redundant with the resolved glyph).
/// When any other modifier is held, Shift is the only thing distinguishing the
/// chord (e.g. `cmd+shift+s` from `cmd+s`) and is kept. See [`KeyInput`].
///
/// Must be applied once, on the complete modifier set (it inspects the whole set
/// rather than clearing a single bit unconditionally).
#[must_use]
pub(crate) fn normalize(key: Key, mods: Modifiers) -> (Key, Modifiers) {
    match key {
        Key::Char(_) if mods.difference(Modifiers::SHIFT).is_empty() => (key, Modifiers::NONE),
        _ => (key, mods),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_union_and_contains() {
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(cs.contains(Modifiers::CTRL));
        assert!(cs.contains(Modifiers::SHIFT));
        assert!(!cs.contains(Modifiers::ALT));
        assert!(cs.contains(Modifiers::CTRL | Modifiers::SHIFT));
    }

    #[test]
    fn modifiers_empty_contains_only_empty() {
        assert!(Modifiers::NONE.is_empty());
        assert!(Modifiers::NONE.contains(Modifiers::NONE));
        assert!(!Modifiers::NONE.contains(Modifiers::CTRL));
        assert!(Modifiers::CTRL.contains(Modifiers::NONE));
    }

    #[test]
    fn modifiers_difference_clears_bits() {
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        assert_eq!(cs.difference(Modifiers::SHIFT), Modifiers::CTRL);
        assert_eq!(cs.difference(Modifiers::ALT), cs);
    }

    #[test]
    fn key_input_is_hash_eq_by_value() {
        let a = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        let b = KeyInput::new(Key::Char('q'), Modifiers::CTRL);
        let c = KeyInput::new(Key::Char('q'), Modifiers::NONE);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
