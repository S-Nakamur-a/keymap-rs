//! The key-string grammar: `FromStr` and `Display` for [`KeyInput`].
//!
//! This is a stable, public compatibility surface — config files contain these
//! strings and the discovery layer prints them. The policy is liberal-in /
//! canonical-out:
//!
//! - `Display` emits exactly one canonical spelling: modifiers in the fixed
//!   order `ctrl`, `alt`, `shift`, `super`, joined with `+`, then the key. Key
//!   names and modifier tokens are lowercase; a character glyph is emitted
//!   verbatim (case-sensitive, since `Char('A')` and `Char('a')` differ).
//! - `FromStr` accepts case-insensitive tokens and aliases (`control`, `cmd`,
//!   `return`, `escape`, …). Adding aliases is backward compatible; removing or
//!   renaming any accepted/emitted spelling is a breaking change.
//!
//! Parsing applies the same [`normalize`](super::normalize) rule as the backend
//! conversion, so `"shift+a"` and `"a"` both parse to `Char('a')` with no
//! modifiers — matching what a terminal actually delivers.

use core::fmt;
use core::str::FromStr;

use super::{Key, KeyInput, Modifiers};

impl fmt::Display for KeyInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mods = self.modifiers();
        if mods.contains(Modifiers::CTRL) {
            f.write_str("ctrl+")?;
        }
        if mods.contains(Modifiers::ALT) {
            f.write_str("alt+")?;
        }
        if mods.contains(Modifiers::SHIFT) {
            f.write_str("shift+")?;
        }
        if mods.contains(Modifiers::SUPER) {
            f.write_str("super+")?;
        }

        // No `_` arm: a newly added `Key` variant must be given a spelling here,
        // or this fails to compile — `Display` is serialization-grade.
        match self.key() {
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "f{n}"),
            Key::Enter => f.write_str("enter"),
            Key::Esc => f.write_str("esc"),
            Key::Tab => f.write_str("tab"),
            Key::Backspace => f.write_str("backspace"),
            Key::Delete => f.write_str("delete"),
            Key::Insert => f.write_str("insert"),
            Key::Home => f.write_str("home"),
            Key::End => f.write_str("end"),
            Key::PageUp => f.write_str("pageup"),
            Key::PageDown => f.write_str("pagedown"),
            Key::Up => f.write_str("up"),
            Key::Down => f.write_str("down"),
            Key::Left => f.write_str("left"),
            Key::Right => f.write_str("right"),
        }
    }
}

impl FromStr for KeyInput {
    type Err = ParseKeyInputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseKeyInputError::new(s, ErrorKind::Empty));
        }

        // Greedily consume leading `modifier+` tokens; the remainder is the key.
        // This leaves a literal `+` (e.g. `"ctrl++"`) as the key correctly.
        let mut rest = s;
        let mut mods = Modifiers::NONE;
        while let Some(idx) = rest.find('+') {
            let token = &rest[..idx];
            if let Some(m) = parse_modifier(token) {
                mods |= m;
                rest = &rest[idx + 1..];
            } else {
                break;
            }
        }

        let key =
            parse_key(rest).ok_or_else(|| ParseKeyInputError::new(s, ErrorKind::InvalidKey))?;
        Ok(KeyInput::normalized(key, mods))
    }
}

fn parse_modifier(token: &str) -> Option<Modifiers> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(Modifiers::CTRL),
        "alt" | "opt" | "option" => Some(Modifiers::ALT),
        "shift" => Some(Modifiers::SHIFT),
        "cmd" | "super" | "win" | "meta" => Some(Modifiers::SUPER),
        _ => None,
    }
}

fn parse_key(token: &str) -> Option<Key> {
    if token.is_empty() {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    let named = match lower.as_str() {
        "tab" => Some(Key::Tab),
        "enter" | "return" => Some(Key::Enter),
        "esc" | "escape" => Some(Key::Esc),
        "space" => Some(Key::Char(' ')),
        "backspace" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "insert" | "ins" => Some(Key::Insert),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        _ => None,
    };
    if let Some(key) = named {
        return Some(key);
    }

    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u8>() {
            if (1..=24).contains(&n) {
                return Some(Key::F(n));
            }
        }
    }

    // A single character key — case preserved (the glyph is significant).
    let mut chars = token.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(Key::Char(c))
}

/// Error returned when a string cannot be parsed into a [`KeyInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeyInputError {
    input: String,
    kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Empty,
    InvalidKey,
}

impl ParseKeyInputError {
    fn new(input: &str, kind: ErrorKind) -> Self {
        ParseKeyInputError {
            input: input.to_string(),
            kind,
        }
    }

    /// The input string that failed to parse.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for ParseKeyInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::Empty => f.write_str("empty key string"),
            ErrorKind::InvalidKey => write!(f, "invalid key string: {:?}", self.input),
        }
    }
}

impl std::error::Error for ParseKeyInputError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trips_through_from_str() {
        // The fixed-point law: parsing a Display'd (already-normalized) value
        // returns it unchanged. A Char carries SHIFT only alongside another
        // modifier (bare Shift on a Char is folded away, so those aren't fixed
        // points and are excluded here).
        let cases = [
            KeyInput::new(Key::Char('a'), Modifiers::NONE),
            KeyInput::new(Key::Char('A'), Modifiers::NONE),
            KeyInput::new(Key::Char('1'), Modifiers::SUPER),
            KeyInput::new(Key::Char('s'), Modifiers::CTRL | Modifiers::SHIFT),
            KeyInput::new(Key::Char('あ'), Modifiers::CTRL),
            KeyInput::new(Key::Char(' '), Modifiers::NONE),
            KeyInput::new(Key::Char('+'), Modifiers::CTRL),
            KeyInput::new(Key::F(1), Modifiers::NONE),
            KeyInput::new(Key::F(12), Modifiers::SHIFT),
            KeyInput::new(Key::Tab, Modifiers::SHIFT),
            KeyInput::new(Key::Esc, Modifiers::NONE),
            KeyInput::new(Key::Up, Modifiers::CTRL | Modifiers::ALT),
            KeyInput::new(
                Key::Enter,
                Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::SUPER,
            ),
        ];
        for k in cases {
            let rendered = k.to_string();
            assert_eq!(
                rendered.parse::<KeyInput>(),
                Ok(k),
                "round trip via {rendered:?}"
            );
        }
    }

    #[test]
    fn display_uses_canonical_modifier_order_and_names() {
        let k = KeyInput::new(Key::Char('a'), Modifiers::SUPER | Modifiers::CTRL);
        assert_eq!(k.to_string(), "ctrl+super+a");
        assert_eq!(KeyInput::new(Key::F(1), Modifiers::NONE).to_string(), "f1");
        assert_eq!(
            KeyInput::new(Key::Tab, Modifiers::SHIFT).to_string(),
            "shift+tab"
        );
    }

    #[test]
    fn from_str_accepts_aliases_case_insensitively() {
        let ctrl_a = KeyInput::new(Key::Char('a'), Modifiers::CTRL);
        assert_eq!("ctrl+a".parse::<KeyInput>().unwrap(), ctrl_a);
        assert_eq!("CTRL+a".parse::<KeyInput>().unwrap(), ctrl_a);
        assert_eq!("control+a".parse::<KeyInput>().unwrap(), ctrl_a);
        assert_eq!(
            "cmd+1".parse::<KeyInput>().unwrap(),
            "super+1".parse::<KeyInput>().unwrap()
        );
    }

    #[test]
    fn from_str_shares_shift_normalization() {
        // Bare Shift on a char is redundant with the glyph, so "shift+a" == "a".
        let a = KeyInput::new(Key::Char('a'), Modifiers::NONE);
        assert_eq!("shift+a".parse::<KeyInput>().unwrap(), a);
        assert_eq!("a".parse::<KeyInput>().unwrap(), a);
    }

    #[test]
    fn from_str_keeps_shift_with_other_modifier() {
        // With another modifier, Shift survives and the chord is distinct —
        // this is what makes cmd+shift+s usable as a binding separate from cmd+s.
        let cmd_shift_s = "cmd+shift+s".parse::<KeyInput>().unwrap();
        assert_eq!(
            cmd_shift_s,
            KeyInput::new(Key::Char('s'), Modifiers::SUPER | Modifiers::SHIFT)
        );
        assert_ne!(cmd_shift_s, "cmd+s".parse::<KeyInput>().unwrap());
        assert_ne!(
            "ctrl+shift+s".parse::<KeyInput>().unwrap(),
            "ctrl+s".parse::<KeyInput>().unwrap()
        );
    }

    #[test]
    fn from_str_handles_literal_plus_as_key() {
        assert_eq!(
            "ctrl++".parse::<KeyInput>().unwrap(),
            KeyInput::new(Key::Char('+'), Modifiers::CTRL)
        );
        assert_eq!(
            "+".parse::<KeyInput>().unwrap(),
            KeyInput::new(Key::Char('+'), Modifiers::NONE)
        );
    }

    #[test]
    fn from_str_parses_named_and_function_keys() {
        assert_eq!("f1".parse::<KeyInput>().unwrap().key(), Key::F(1));
        assert_eq!("up".parse::<KeyInput>().unwrap().key(), Key::Up);
        assert_eq!("esc".parse::<KeyInput>().unwrap().key(), Key::Esc);
        assert_eq!("escape".parse::<KeyInput>().unwrap().key(), Key::Esc);
    }

    #[test]
    fn from_str_rejects_invalid_input() {
        assert!("".parse::<KeyInput>().is_err());
        assert!("ctrl".parse::<KeyInput>().is_err());
        assert!("hyper+a".parse::<KeyInput>().is_err());
        assert!("ctrl+xyz".parse::<KeyInput>().is_err());
    }
}
