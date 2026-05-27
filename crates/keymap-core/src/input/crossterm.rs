//! Conversion from `crossterm` key events into the neutral [`KeyInput`].
//!
//! This is the only place a `crossterm` type touches `keymap-core`, and it is
//! compiled only under the `crossterm` feature. Keeping the adapter here (rather
//! than letting `crossterm::event::KeyEvent` appear in always-compiled
//! signatures) is what decouples crossterm's version from this crate's semver
//! for default builds.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Key, KeyInput, Modifiers};

/// Error returned when a `crossterm` key has no representation in [`Key`] yet.
///
/// `keymap-core` only enumerates keys whose identity is unambiguous across
/// environments; media keys, lock keys, lone modifier presses, and similar are
/// reported here instead of being silently dropped or guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedKey {
    code: KeyCode,
}

impl UnsupportedKey {
    /// The `crossterm` key code that could not be converted.
    #[must_use]
    pub fn code(&self) -> KeyCode {
        self.code
    }
}

impl core::fmt::Display for UnsupportedKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "unsupported key code: {:?}", self.code)
    }
}

impl std::error::Error for UnsupportedKey {}

/// Maps crossterm's modifier bits onto ours (including Shift; the shared
/// [`normalize`](super::normalize) rule decides whether to keep it).
fn modifiers_of(km: KeyModifiers) -> Modifiers {
    let mut m = Modifiers::NONE;
    if km.contains(KeyModifiers::CONTROL) {
        m |= Modifiers::CTRL;
    }
    if km.contains(KeyModifiers::ALT) {
        m |= Modifiers::ALT;
    }
    if km.contains(KeyModifiers::SUPER) {
        m |= Modifiers::SUPER;
    }
    if km.contains(KeyModifiers::SHIFT) {
        m |= Modifiers::SHIFT;
    }
    m
}

impl TryFrom<KeyEvent> for KeyInput {
    type Error = UnsupportedKey;

    fn try_from(event: KeyEvent) -> Result<Self, Self::Error> {
        let mut mods = modifiers_of(event.modifiers);

        let key = match event.code {
            KeyCode::Char(c) => Key::Char(c),
            // BackTab is Shift+Tab; normalize to Tab carrying SHIFT.
            KeyCode::BackTab => {
                mods |= Modifiers::SHIFT;
                Key::Tab
            }
            KeyCode::F(n) => Key::F(n),
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Insert => Key::Insert,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            other => return Err(UnsupportedKey { code: other }),
        };

        // Shared rule: drops SHIFT for character keys, keeps it otherwise.
        Ok(KeyInput::normalized(key, mods))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_is_dropped_for_resolved_glyph() {
        // Terminal already resolved the glyph to 'A'; we only clear SHIFT.
        let event = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Char('A'));
        assert_eq!(input.modifiers(), Modifiers::NONE);
    }

    #[test]
    fn glyph_is_trusted_not_recased() {
        // A base-layout report ('a' + SHIFT) is NOT re-cased to 'A': we cannot
        // know the layout from the glyph. SHIFT is dropped, glyph kept as-is.
        // Documented limitation: this won't match a Char('A') binding.
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SHIFT);
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Char('a'));
        assert_eq!(input.modifiers(), Modifiers::NONE);
    }

    #[test]
    fn ctrl_char_keeps_ctrl() {
        let event = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Char('q'));
        assert_eq!(input.modifiers(), Modifiers::CTRL);
    }

    #[test]
    fn ctrl_shift_char_keeps_shift_as_distinct_chord() {
        // With another modifier present, SHIFT is the only signal separating
        // this chord from Ctrl+s, so it must survive (not be folded away).
        let event = KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Char('s'));
        assert_eq!(input.modifiers(), Modifiers::CTRL | Modifiers::SHIFT);

        // Distinct from Ctrl+s.
        let ctrl_s =
            KeyInput::try_from(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        assert_ne!(input, ctrl_s);
    }

    #[test]
    fn alt_and_super_pass_through_on_char() {
        let alt = KeyInput::try_from(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)).unwrap();
        assert_eq!(alt.modifiers(), Modifiers::ALT);

        // SUPER on a digit key (cmd+1) — the modifier survives the char path.
        let sup =
            KeyInput::try_from(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::SUPER)).unwrap();
        assert_eq!(sup.key(), Key::Char('1'));
        assert_eq!(sup.modifiers(), Modifiers::SUPER);
    }

    #[test]
    fn non_ascii_char_passes_through_with_shift_dropped() {
        let event = KeyEvent::new(KeyCode::Char('あ'), KeyModifiers::SHIFT);
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Char('あ'));
        assert_eq!(input.modifiers(), Modifiers::NONE);
    }

    #[test]
    fn backtab_and_shift_tab_converge_to_same_input() {
        // Two distinct crossterm encodings of the same logical key must
        // normalize to one KeyInput, or HashMap lookup silently misses.
        let from_backtab =
            KeyInput::try_from(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)).unwrap();
        let from_shift_tab =
            KeyInput::try_from(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(from_backtab, from_shift_tab);
    }

    #[test]
    fn backtab_normalizes_to_shift_tab() {
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Tab);
        assert_eq!(input.modifiers(), Modifiers::SHIFT);
    }

    #[test]
    fn shift_kept_for_non_char_keys() {
        let event = KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT);
        let input = KeyInput::try_from(event).unwrap();
        assert_eq!(input.key(), Key::Up);
        assert_eq!(input.modifiers(), Modifiers::SHIFT);
    }

    #[test]
    fn unsupported_key_is_reported_not_guessed() {
        let event = KeyEvent::new(KeyCode::CapsLock, KeyModifiers::NONE);
        let err = KeyInput::try_from(event).unwrap_err();
        assert_eq!(err.code(), KeyCode::CapsLock);
    }
}
