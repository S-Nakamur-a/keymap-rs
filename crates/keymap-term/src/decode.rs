//! Capability-aware decoding of raw terminal bytes into a [`KeyInput`].
//!
//! This is the empirical counterpart to `keymap_core::KeyInput::legacy_form`:
//! where `legacy_form` is a *terminal-independent* static fact, this decoder
//! turns the bytes a terminal *actually sent* (recorded in `captures/*.toml`)
//! back into the neutral key vocabulary. It is built **measurement-first** — the
//! cases it handles are exactly the byte shapes the committed captures contain
//! (single C0 control bytes, `ESC`-prefixed CSI / SS3 sequences, `ESC`+key Meta
//! prefixes, and UTF-8 glyphs), plus their obvious near-neighbours. Byte forms no
//! recorded terminal has produced — notably the kitty progressive-enhancement
//! encodings — are deliberately *not* invented here; they wait for their own
//! captures (the enhanced re-capture follow-up in `docs/TESTING.md`).
//!
//! # State-free, one press per front of the slice
//!
//! Consistent with the rest of `keymap-rs`, the decoder owns no state and no
//! clock. It is a pure function of `(bytes, mode)`: it decodes the *first*
//! complete key press at the front of `bytes` and reports how many bytes that
//! press consumed, so a caller streaming bytes can advance its own buffer. The
//! caller owns that buffer and any inter-byte timeout. This is why a lone `ESC`
//! (`0x1b`) decodes to [`Key::Esc`] rather than blocking: the decoder cannot see
//! the future, so it treats the slice it is given as settled. When the slice is a
//! genuine *prefix* of a longer encoding (a truncated CSI, an incomplete UTF-8
//! sequence), it returns [`Decoded::Incomplete`] so the caller knows to read
//! more rather than to treat the bytes as a miss.
//!
//! # Trust posture
//!
//! Decoded keys are **untrusted**. When fed bytes from a PTY, a hostile process
//! running inside the terminal can emit any byte sequence, so *any* baseline
//! chord — including `ctrl+c`, a detach key, or whatever the host treats as
//! reserved — can be forged. The decoder is byte-faithful by design and does
//! **no** trust filtering or reserved-key special-casing; it cannot tell a real
//! keypress from injected bytes, and must not try. Reserved-key enforcement is a
//! *resolve-time, caller-composition* concern: bind reserved keys in the
//! outermost layer so they are reached first (see "escape hatch is by ordering"
//! in `docs/STATUS.md`). A decoded [`Key::Char`] carries the literal Unicode
//! codepoint with no case-folding, so a homoglyph (Cyrillic `а` vs ASCII `a`) is
//! a *distinct* key and cannot alias another binding.

use keymap_core::{Key, KeyInput, Modifiers};

/// Which terminal key-encoding mode produced the bytes being decoded.
///
/// Progressive enhancement *adds new byte shapes* for chords that the legacy
/// encoding cannot represent or disambiguate; it does not reinterpret the shapes
/// the two modes share. In [`Baseline`], `ctrl+i` is indistinguishable from `Tab`
/// (both arrive as `0x09`); a [`KittyEnhanced`] terminal disambiguates them by
/// sending `ctrl+i` as a distinct CSI-u sequence (`ESC [ 105 ; 5 u`) while `Tab`
/// still arrives as `0x09`. So the shared bytes keep their baseline meaning and
/// [`KittyEnhanced`] recognises a *superset* of shapes. The enum is
/// `#[non_exhaustive]`: further capability modes can be added without breaking.
///
/// This is a *runtime decode input*, distinct from the on-disk
/// [`Capability`](crate::Capability) DTO: that records *what was observed about a
/// terminal*; this selects *how to read the bytes*.
///
/// [`Baseline`]: DecodeMode::Baseline
/// [`KittyEnhanced`]: DecodeMode::KittyEnhanced
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeMode {
    /// Legacy 7-bit C0 / xterm encoding, with no progressive-enhancement flags
    /// pushed. This is what the baseline captures were recorded in.
    Baseline,
    /// The kitty keyboard protocol pushed with `DISAMBIGUATE_ESCAPE_CODES` (the
    /// flag `keymap-tui`'s F3 toggles and `keymap-probe --enhanced` records).
    /// On top of every baseline shape it recognises the kitty CSI-u form
    /// (`ESC [ <codepoint> [; <mods>] u`) and function keys sent as CSI letters
    /// (`ESC [ P` … `ESC [ S` for `F1`–`F4`). Built from the committed
    /// `*_kitty-enhanced_*.toml` captures.
    KittyEnhanced,
}

impl DecodeMode {
    /// Maps a capture's [`KeyPress::mode`](crate::KeyPress::mode) string onto the
    /// typed decode mode, returning `None` for a mode this decoder cannot yet
    /// read (so an unrecorded mode is an explicit miss, never a silent
    /// mis-decode under baseline rules).
    #[must_use]
    pub fn from_capture_mode(mode: &str) -> Option<DecodeMode> {
        match mode {
            "baseline" => Some(DecodeMode::Baseline),
            "kitty-enhanced" => Some(DecodeMode::KittyEnhanced),
            _ => None,
        }
    }
}

/// The outcome of decoding the front of a byte slice.
///
/// Unlike `keymap_seq::Match` (a trie lookup with a *provably closed* set of
/// three outcomes, hence exhaustive), the set of decode outcomes is **open**: the
/// capture matrix grows, and future capability modes can surface byte forms — and
/// possibly outcomes — that the baseline does not. So this enum is
/// `#[non_exhaustive]`; downstream `match`es need a `_` arm, which should err
/// toward "keep reading / pass through" for an outcome it does not recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[must_use]
pub enum Decoded {
    /// A complete key press was decoded from the first `consumed` bytes of the
    /// input. `consumed` lets a streaming caller advance its buffer (and is the
    /// exact byte span a future PTY passthrough would copy verbatim).
    Key {
        /// The normalized key press.
        input: KeyInput,
        /// How many leading bytes this press consumed (`>= 1`).
        consumed: usize,
    },
    /// The bytes are a valid *prefix* of one or more known encodings but do not
    /// yet form a complete press (empty input, a truncated CSI/SS3 sequence, or
    /// an incomplete UTF-8 sequence). The caller should read more bytes. Whether
    /// "more" will ever arrive is the caller's clock to keep — the decoder has
    /// none.
    Incomplete,
    /// The bytes match no encoding this mode knows. The caller should not treat
    /// them as a key; how far to resynchronise is the caller's policy.
    Unrecognized,
}

/// Decodes the first complete key press at the front of `bytes`, read under the
/// given [`DecodeMode`]. See the [module docs](self) for the contract.
pub fn decode(bytes: &[u8], mode: DecodeMode) -> Decoded {
    decode_press(bytes, mode)
}

/// Builds a [`Decoded::Key`] with the shared normalization rule applied.
fn decoded(key: Key, mods: Modifiers, consumed: usize) -> Decoded {
    Decoded::Key {
        input: KeyInput::normalized(key, mods),
        consumed,
    }
}

/// Decodes one press from the front of `bytes`. This is the shared core of every
/// mode: only the CSI interpretation (reached via [`decode_escape`]) is
/// mode-sensitive, which is exactly the span of the measured baseline/enhanced
/// divergence — every other byte shape decodes identically in both modes.
fn decode_press(bytes: &[u8], mode: DecodeMode) -> Decoded {
    let Some(&first) = bytes.first() else {
        // Empty input is the prefix of everything — feed me bytes.
        return Decoded::Incomplete;
    };
    match first {
        0x1b => decode_escape(bytes, mode),
        // `0x7f` (DEL) is the Backspace key on most terminals.
        0x7f => decoded(Key::Backspace, Modifiers::NONE, 1),
        // Other C0 control bytes: Ctrl chords and the named-key C0 aliases.
        b if b < 0x20 => decode_c0(b),
        // Printable ASCII or a UTF-8 glyph.
        _ => decode_utf8(bytes),
    }
}

/// Decodes a single C0 control byte (`0x00..=0x1a`, `0x1c..=0x1f`; `0x1b` is
/// handled on the escape path). The legacy C0 aliases (`ctrl+i` ≡ `Tab`,
/// `ctrl+m` ≡ `Enter`) collapse here exactly as `KeyInput::legacy_form` predicts.
fn decode_c0(b: u8) -> Decoded {
    match b {
        0x09 => decoded(Key::Tab, Modifiers::NONE, 1), // ctrl+i shares this byte
        0x0d => decoded(Key::Enter, Modifiers::NONE, 1), // ctrl+m shares this byte
        // Ctrl + letter: 0x01 -> 'a' … 0x1a -> 'z' (Tab/Enter handled above).
        0x01..=0x1a => {
            let c = (b - 0x01 + b'a') as char;
            decoded(Key::Char(c), Modifiers::CTRL, 1)
        }
        // NUL (ctrl+@/ctrl+space) and FS/GS/RS/US are ambiguous and unrecorded.
        _ => Decoded::Unrecognized,
    }
}

/// Decodes an `ESC`-introduced byte group (`bytes[0] == 0x1b`).
fn decode_escape(bytes: &[u8], mode: DecodeMode) -> Decoded {
    match bytes.get(1) {
        // Lone ESC: a complete Escape press (the caller's timeout, not ours,
        // distinguishes this from the start of a sequence that hasn't arrived).
        None => decoded(Key::Esc, Modifiers::NONE, 1),
        Some(0x5b) => decode_csi(bytes, mode),
        Some(0x4f) => decode_ss3(bytes),
        // ESC + another control byte (e.g. Alt+Ctrl chords) is unrecorded.
        Some(&b) if b < 0x20 => Decoded::Unrecognized,
        // ESC + a key is the Meta / Alt-as-prefix convention.
        Some(_) => decode_alt(bytes, mode),
    }
}

/// The structural result of scanning a CSI body (everything after `ESC [`). This
/// is pure byte-grammar with no mode opinion — the two modes share it and only
/// *interpret* the result differently — so the subtle abort/incomplete
/// trichotomy lives in exactly one place.
enum CsiScan<'a> {
    /// A final byte (`0x40..=0x7e`) was reached. `params` is the
    /// parameter/intermediate run that preceded it (empty for an unparameterised
    /// CSI).
    Complete {
        params: &'a [u8],
        final_byte: u8,
        consumed: usize,
    },
    /// Ran out of bytes inside a legal param/intermediate run — a truncated CSI,
    /// read more.
    Incomplete,
    /// An illegal body byte (a C0 control, an embedded `ESC`, a byte above the GL
    /// range) aborted the sequence: no future byte can complete it.
    Unrecognized,
}

/// Scans the CSI body. Only parameter (`0x30..=0x3f`) and intermediate
/// (`0x20..=0x2f`) bytes may precede the final byte; anything else aborts rather
/// than falsely promising "read more".
fn scan_csi(bytes: &[u8]) -> CsiScan<'_> {
    let body = &bytes[2..];
    for (i, &b) in body.iter().enumerate() {
        if (0x40..=0x7e).contains(&b) {
            return CsiScan::Complete {
                params: &body[..i],
                final_byte: b,
                consumed: 2 + i + 1,
            };
        }
        if !(0x20..=0x3f).contains(&b) {
            return CsiScan::Unrecognized;
        }
    }
    CsiScan::Incomplete
}

/// Decodes a CSI sequence (`ESC [ …`), interpreting the shared [`scan_csi`]
/// result per [`DecodeMode`].
fn decode_csi(bytes: &[u8], mode: DecodeMode) -> Decoded {
    match scan_csi(bytes) {
        CsiScan::Complete {
            params,
            final_byte,
            consumed,
        } => match mode {
            DecodeMode::Baseline => interpret_csi_baseline(params, final_byte, consumed),
            DecodeMode::KittyEnhanced => interpret_csi_enhanced(params, final_byte, consumed),
        },
        CsiScan::Incomplete => Decoded::Incomplete,
        CsiScan::Unrecognized => Decoded::Unrecognized,
    }
}

/// Baseline CSI: only the unparameterised arrow/BackTab forms. A parameterised
/// CSI (`ESC [ 1 ; 2 A`, the kitty/`modifyOtherKeys` shape) is an enhanced shape
/// and is [`Unrecognized`](Decoded::Unrecognized) here.
fn interpret_csi_baseline(params: &[u8], final_byte: u8, consumed: usize) -> Decoded {
    if !params.is_empty() {
        return Decoded::Unrecognized;
    }
    match final_byte {
        b'A' => decoded(Key::Up, Modifiers::NONE, consumed),
        b'B' => decoded(Key::Down, Modifiers::NONE, consumed),
        b'C' => decoded(Key::Right, Modifiers::NONE, consumed),
        b'D' => decoded(Key::Left, Modifiers::NONE, consumed),
        b'Z' => decoded(Key::Tab, Modifiers::SHIFT, consumed), // BackTab
        _ => Decoded::Unrecognized,
    }
}

/// Enhanced CSI: the baseline forms *plus* the kitty CSI-u sequence (final `u`)
/// and function keys sent as CSI letters (`P`–`S` → `F1`–`F4`). Everything else
/// delegates to [`interpret_csi_baseline`], so enhanced is a strict superset that
/// only *adds* recognitions where baseline reports `Unrecognized`.
fn interpret_csi_enhanced(params: &[u8], final_byte: u8, consumed: usize) -> Decoded {
    match final_byte {
        b'u' => decode_csi_u(params, consumed),
        // F1–F4 as bare CSI letters (kitty sends `ESC [ P` rather than the
        // baseline SS3 `ESC O P`). A parameterised (modified) F-key is an
        // unmeasured shape, so it falls through to baseline → `Unrecognized`.
        // `P`=F1 … `S`=F4: `b'P'` is the F1 offset, F-keys are 1-based.
        b'P'..=b'S' if params.is_empty() => {
            decoded(Key::F(1 + (final_byte - b'P')), Modifiers::NONE, consumed)
        }
        _ => interpret_csi_baseline(params, final_byte, consumed),
    }
}

/// Decodes a kitty CSI-u body (`ESC [ <codepoint> [; <mods>] u`). The `params`
/// are the bytes [`scan_csi`] gathered before the `u`. Malformed input — empty or
/// non-decimal fields, a codepoint that is not a Unicode scalar value, a modifier
/// value below the protocol's `1` floor, or an unmeasured extra field — is
/// [`Unrecognized`](Decoded::Unrecognized) rather than a guess: these are
/// untrusted bytes.
fn decode_csi_u(params: &[u8], consumed: usize) -> Decoded {
    match parse_csi_u(params) {
        Some((key, mods)) => decoded(key, mods, consumed),
        None => Decoded::Unrecognized,
    }
}

/// Parses the kitty CSI-u parameter body into `(key, modifiers)`, or `None` if it
/// is malformed or carries a richer (unmeasured) CSI-u sub-grammar.
fn parse_csi_u(params: &[u8]) -> Option<(Key, Modifiers)> {
    let mut fields = params.split(|&b| b == b';');
    let codepoint = parse_u32_decimal(fields.next()?)?;
    let key = kitty_key_from_codepoint(codepoint)?;
    let mods = match fields.next() {
        None => Modifiers::NONE,
        Some(field) => kitty_modifiers(parse_u32_decimal(field)?)?,
    };
    // A third `;`-separated field is a richer CSI-u shape (alternate keycodes,
    // associated text) that no capture exercises; refuse rather than silently
    // drop it from untrusted bytes.
    if fields.next().is_some() {
        return None;
    }
    Some((key, mods))
}

/// Parses an ASCII-decimal byte run into a `u32`. `None` on an empty run, a
/// non-digit byte, or overflow — allocation- and panic-free.
fn parse_u32_decimal(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0u32, |acc, &b| {
        let digit = char::from(b).to_digit(10)?;
        acc.checked_mul(10)?.checked_add(digit)
    })
}

/// Maps a kitty CSI-u codepoint to a [`Key`]: the functional codepoints reuse
/// their ASCII control values (`9` → `Tab`, `27` → `Esc`, and the near-neighbour
/// `13` → `Enter`, `127` → `Backspace`); any other codepoint is its glyph.
/// `None` for a value that is not a Unicode scalar (a surrogate or `> U+10FFFF`).
fn kitty_key_from_codepoint(codepoint: u32) -> Option<Key> {
    Some(match codepoint {
        9 => Key::Tab,
        13 => Key::Enter,
        27 => Key::Esc,
        127 => Key::Backspace,
        other => Key::Char(char::from_u32(other)?),
    })
}

/// Decodes a kitty modifier value into [`Modifiers`]. kitty encodes it as
/// `1 + bitfield` (shift=1, alt=2, ctrl=4, super=8), so a value below `1` is
/// illegal and rejected (`None`). Unknown high bits (hyper / meta / lock states)
/// have no neutral-vocabulary equivalent and are ignored, degrading to the chord
/// we *can* represent rather than dropping the press.
fn kitty_modifiers(value: u32) -> Option<Modifiers> {
    const TABLE: [(u32, Modifiers); 4] = [
        (0b0001, Modifiers::SHIFT),
        (0b0010, Modifiers::ALT),
        (0b0100, Modifiers::CTRL),
        (0b1000, Modifiers::SUPER),
    ];
    let bits = value.checked_sub(1)?;
    Some(TABLE.iter().fold(Modifiers::NONE, |acc, &(bit, m)| {
        if bits & bit != 0 { acc.union(m) } else { acc }
    }))
}

/// Decodes an SS3 sequence (`ESC O x`), which baseline terminals use for `F1`–`F4`.
fn decode_ss3(bytes: &[u8]) -> Decoded {
    match bytes.get(2) {
        None => Decoded::Incomplete, // ESC O <missing final byte>
        Some(b) => match b {
            b'P' => decoded(Key::F(1), Modifiers::NONE, 3),
            b'Q' => decoded(Key::F(2), Modifiers::NONE, 3),
            b'R' => decoded(Key::F(3), Modifiers::NONE, 3),
            b'S' => decoded(Key::F(4), Modifiers::NONE, 3),
            _ => Decoded::Unrecognized,
        },
    }
}

/// Decodes an `ESC`+key Meta prefix: decode the payload as a standalone press and
/// add [`Modifiers::ALT`]. The payload is decoded in the *same* mode, so a Meta
/// prefix on an enhanced payload resolves under the enhanced rules.
fn decode_alt(bytes: &[u8], mode: DecodeMode) -> Decoded {
    match decode_press(&bytes[1..], mode) {
        Decoded::Key { input, consumed } => Decoded::Key {
            input: KeyInput::normalized(input.key(), input.modifiers().union(Modifiers::ALT)),
            consumed: consumed + 1,
        },
        // A truncated or unrecognised payload propagates unchanged.
        other => other,
    }
}

/// Decodes a leading printable / UTF-8 byte sequence into a `Char`.
fn decode_utf8(bytes: &[u8]) -> Decoded {
    let Some(len) = utf8_len(bytes[0]) else {
        return Decoded::Unrecognized; // a stray continuation or invalid lead byte
    };
    if bytes.len() < len {
        // Only a prefix that could still complete is `Incomplete`. If a
        // continuation byte already present is invalid, no future byte rescues
        // the sequence, so it is `Unrecognized`, not "read more".
        return if bytes[1..].iter().all(|b| (0x80..=0xbf).contains(b)) {
            Decoded::Incomplete
        } else {
            Decoded::Unrecognized
        };
    }
    match core::str::from_utf8(&bytes[..len]) {
        // `utf8_len` fixed the length from the lead byte, so the first char of a
        // valid decode spans exactly those bytes.
        Ok(s) => match s.chars().next() {
            Some(c) => decoded(Key::Char(c), Modifiers::NONE, len),
            None => Decoded::Unrecognized,
        },
        Err(_) => Decoded::Unrecognized, // bad continuation bytes
    }
}

/// The length of a UTF-8 sequence from its leading byte, or `None` if `first` is
/// not a valid lead byte (a continuation byte or an invalid value).
fn utf8_len(first: u8) -> Option<usize> {
    match first {
        0x00..=0x7f => Some(1),
        0xc0..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf7 => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: the `Decoded::Key` for `(key, mods)` consuming `n` bytes.
    fn key(key: Key, mods: Modifiers, n: usize) -> Decoded {
        decoded(key, mods, n)
    }

    fn base(bytes: &[u8]) -> Decoded {
        decode(bytes, DecodeMode::Baseline)
    }

    fn enh(bytes: &[u8]) -> Decoded {
        decode(bytes, DecodeMode::KittyEnhanced)
    }

    // --- The committed-fixture byte shapes (mirrored as unit tests so the
    //     decode rules are documented next to the code; the fixture-replay test
    //     in tests/ asserts them against the real capture files). ---

    #[test]
    fn ctrl_letter_c0_byte() {
        assert_eq!(base(&[0x01]), key(Key::Char('a'), Modifiers::CTRL, 1));
    }

    #[test]
    fn ctrl_i_decodes_to_tab_in_baseline() {
        // The documented ambiguity: ctrl+i is indistinguishable from Tab here.
        assert_eq!(base(&[0x09]), key(Key::Tab, Modifiers::NONE, 1));
    }

    #[test]
    fn ctrl_m_decodes_to_enter() {
        assert_eq!(base(&[0x0d]), key(Key::Enter, Modifiers::NONE, 1));
    }

    #[test]
    fn lone_esc_is_a_complete_escape_press() {
        assert_eq!(base(&[0x1b]), key(Key::Esc, Modifiers::NONE, 1));
    }

    #[test]
    fn csi_arrows() {
        assert_eq!(base(&[0x1b, 0x5b, b'A']), key(Key::Up, Modifiers::NONE, 3));
        assert_eq!(
            base(&[0x1b, 0x5b, b'B']),
            key(Key::Down, Modifiers::NONE, 3)
        );
        assert_eq!(
            base(&[0x1b, 0x5b, b'C']),
            key(Key::Right, Modifiers::NONE, 3)
        );
        assert_eq!(
            base(&[0x1b, 0x5b, b'D']),
            key(Key::Left, Modifiers::NONE, 3)
        );
    }

    #[test]
    fn csi_z_is_backtab() {
        assert_eq!(
            base(&[0x1b, 0x5b, 0x5a]),
            key(Key::Tab, Modifiers::SHIFT, 3)
        );
    }

    #[test]
    fn ss3_function_keys() {
        assert_eq!(
            base(&[0x1b, 0x4f, b'P']),
            key(Key::F(1), Modifiers::NONE, 3)
        );
        assert_eq!(
            base(&[0x1b, 0x4f, b'S']),
            key(Key::F(4), Modifiers::NONE, 3)
        );
    }

    #[test]
    fn esc_prefix_is_alt() {
        // Alt-as-Meta camp: ESC + 'a' => alt+a.
        assert_eq!(base(&[0x1b, 0x61]), key(Key::Char('a'), Modifiers::ALT, 2));
    }

    #[test]
    fn option_composed_glyph_loses_the_modifier() {
        // Option-composes camp: alt+a arrives as UTF-8 'å' with NO modifier —
        // the empirical proof that alt+a is not reachable as alt+a here.
        assert_eq!(base(&[0xc3, 0xa5]), key(Key::Char('å'), Modifiers::NONE, 2));
    }

    #[test]
    fn ascii_printable() {
        assert_eq!(base(b"a"), key(Key::Char('a'), Modifiers::NONE, 1));
        assert_eq!(base(b"1"), key(Key::Char('1'), Modifiers::NONE, 1));
    }

    // --- Boundary / abnormal inputs the clean fixtures never exercise. ---

    #[test]
    fn empty_is_incomplete() {
        assert_eq!(base(&[]), Decoded::Incomplete);
    }

    #[test]
    fn truncated_csi_is_incomplete() {
        assert_eq!(base(&[0x1b, 0x5b]), Decoded::Incomplete); // ESC [ <no final>
        assert_eq!(base(&[0x1b, 0x5b, b'1']), Decoded::Incomplete); // ESC [ 1 <no final>
    }

    #[test]
    fn truncated_ss3_is_incomplete() {
        assert_eq!(base(&[0x1b, 0x4f]), Decoded::Incomplete);
    }

    #[test]
    fn parameterised_csi_is_unrecognized_in_baseline() {
        // ESC [ 1 ; 2 A (shift+up) is the enhanced-layer shape; baseline can't.
        assert_eq!(
            base(&[0x1b, 0x5b, b'1', b';', b'2', b'A']),
            Decoded::Unrecognized
        );
    }

    #[test]
    fn unknown_csi_final_is_unrecognized() {
        assert_eq!(base(&[0x1b, 0x5b, b'q']), Decoded::Unrecognized);
    }

    #[test]
    fn truncated_utf8_is_incomplete() {
        assert_eq!(base(&[0xc3]), Decoded::Incomplete); // lead of 'å' without its tail
    }

    #[test]
    fn invalid_utf8_is_unrecognized() {
        assert_eq!(base(&[0xff]), Decoded::Unrecognized); // not a valid lead byte
        assert_eq!(base(&[0xa5]), Decoded::Unrecognized); // a stray continuation byte
        assert_eq!(base(&[0xc3, 0x28]), Decoded::Unrecognized); // bad continuation
    }

    #[test]
    fn truncated_utf8_with_invalid_continuation_is_unrecognized() {
        // `e0` leads a 3-byte sequence but `28` is not a continuation byte, so no
        // future byte can complete it — `Unrecognized`, not `Incomplete`.
        assert_eq!(base(&[0xe0, 0x28]), Decoded::Unrecognized);
        // A genuinely truncated valid prefix is still `Incomplete`.
        assert_eq!(base(&[0xf0, 0x9f, 0x98]), Decoded::Incomplete);
    }

    #[test]
    fn csi_aborted_by_illegal_body_byte_is_unrecognized() {
        // An embedded ESC starts a fresh sequence; scanning past it would merge
        // two presses into one wrong key.
        assert_eq!(base(&[0x1b, 0x5b, 0x1b, 0x5b, b'A']), Decoded::Unrecognized);
        // A C0 control byte is not a legal CSI body byte.
        assert_eq!(base(&[0x1b, 0x5b, 0x09, b'A']), Decoded::Unrecognized);
        // A byte above the GL range can never be a final byte; do not promise
        // "read more" forever.
        assert_eq!(base(&[0x1b, 0x5b, 0x80]), Decoded::Unrecognized);
    }

    #[test]
    fn nul_and_separator_controls_are_unrecognized() {
        // NUL (ctrl+@/ctrl+space) and FS/GS/RS/US have no unambiguous baseline
        // key; they are explicit misses rather than guesses.
        for b in [0x00, 0x1c, 0x1d, 0x1e, 0x1f] {
            assert_eq!(base(&[b]), Decoded::Unrecognized, "byte {b:#04x}");
        }
    }

    #[test]
    fn bs_and_del_are_split_deliberately() {
        // `0x7f` (DEL) is the Backspace key; `0x08` (BS) stays ctrl+h, matching
        // what `legacy_form` predicts for ctrl+h (Representable, not Backspace).
        // This pins the asymmetry so a future "fix" of 0x08 fails loudly.
        assert_eq!(base(&[0x08]), key(Key::Char('h'), Modifiers::CTRL, 1));
        assert_eq!(base(&[0x7f]), key(Key::Backspace, Modifiers::NONE, 1));
    }

    #[test]
    fn esc_flood_does_not_recurse_unboundedly() {
        // A run of ESC bytes must not drive `decode_alt` recursion: the second
        // ESC is a control byte, caught before any recursive descent.
        assert_eq!(base(&[0x1b; 1000]), Decoded::Unrecognized);
    }

    #[test]
    fn trailing_bytes_are_left_for_the_caller() {
        // One Up arrow then a stray 'a': decode the arrow, report consumed=3.
        assert_eq!(
            base(&[0x1b, 0x5b, b'A', b'a']),
            key(Key::Up, Modifiers::NONE, 3)
        );
    }

    #[test]
    fn chunk_boundaries_do_not_affect_the_concatenated_decode() {
        // read_chunks is recorder provenance, not a decoder input: the decoder
        // sees only the concatenated bytes.
        let whole = [0x1b, 0x5b, b'A'];
        assert_eq!(base(&whole), key(Key::Up, Modifiers::NONE, 3));
    }

    #[test]
    fn capture_modes_map_to_typed_decode_modes() {
        assert_eq!(
            DecodeMode::from_capture_mode("baseline"),
            Some(DecodeMode::Baseline)
        );
        assert_eq!(
            DecodeMode::from_capture_mode("kitty-enhanced"),
            Some(DecodeMode::KittyEnhanced)
        );
        // An unrecorded mode is an explicit miss, never a silent mis-decode.
        assert_eq!(DecodeMode::from_capture_mode("kitty-future"), None);
    }

    // --- Kitty-enhanced mode: the committed `*_kitty-enhanced_*.toml` shapes,
    //     mirrored here next to the code (the fixture-replay test asserts them
    //     against the real files). ---

    #[test]
    fn enh_ctrl_letter_is_csi_u_not_c0() {
        // ctrl+a arrives as `ESC [ 97 ; 5 u` (codepoint 97 = 'a', mod 5 = 1+ctrl),
        // *not* the baseline C0 byte 0x01.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x35, 0x75]),
            key(Key::Char('a'), Modifiers::CTRL, 7)
        );
    }

    #[test]
    fn enh_ctrl_i_is_distinct_from_tab() {
        // The whole point of enhancement: ctrl+i = `ESC [ 105 ; 5 u` decodes to
        // Char('i')+CTRL, no longer colliding with Tab (0x09) as in baseline.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x31, 0x30, 0x35, 0x3b, 0x35, 0x75]),
            key(Key::Char('i'), Modifiers::CTRL, 8)
        );
    }

    #[test]
    fn enh_shift_tab_is_functional_codepoint_9() {
        // `ESC [ 9 ; 2 u`: codepoint 9 → Tab, mod 2 = 1+shift.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x3b, 0x32, 0x75]),
            key(Key::Tab, Modifiers::SHIFT, 6)
        );
    }

    #[test]
    fn enh_esc_is_csi_u_codepoint_27_no_modifier() {
        // `ESC [ 27 u`: a CSI-u with no `;mods` field is an unmodified press.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x32, 0x37, 0x75]),
            key(Key::Esc, Modifiers::NONE, 5)
        );
    }

    #[test]
    fn enh_explicit_modifier_one_is_no_modifiers() {
        // kitty encodes modifiers as 1 + bitfield, so a literal `;1` (bitfield 0)
        // means no modifiers — the same as omitting the field.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x31, 0x75]),
            key(Key::Char('a'), Modifiers::NONE, 7)
        );
    }

    #[test]
    fn enh_function_keys_arrive_as_csi_letters() {
        // kitty sends F1–F4 as `ESC [ P … S`, not the baseline SS3 `ESC O P`.
        assert_eq!(enh(&[0x1b, 0x5b, b'P']), key(Key::F(1), Modifiers::NONE, 3));
        assert_eq!(enh(&[0x1b, 0x5b, b'Q']), key(Key::F(2), Modifiers::NONE, 3));
        assert_eq!(enh(&[0x1b, 0x5b, b'R']), key(Key::F(3), Modifiers::NONE, 3));
        assert_eq!(enh(&[0x1b, 0x5b, b'S']), key(Key::F(4), Modifiers::NONE, 3));
        // `ESC [ T` is the near-neighbour that is *not* an F-key.
        assert_eq!(enh(&[0x1b, 0x5b, b'T']), Decoded::Unrecognized);
    }

    #[test]
    fn enh_arrows_are_still_the_baseline_csi_form() {
        // Unparameterised arrows are byte-identical to baseline in enhanced mode.
        assert_eq!(enh(&[0x1b, 0x5b, b'A']), key(Key::Up, Modifiers::NONE, 3));
        assert_eq!(enh(&[0x1b, 0x5b, b'D']), key(Key::Left, Modifiers::NONE, 3));
    }

    #[test]
    fn enh_alt_a_diverges_by_terminal_just_like_baseline() {
        // kitty composes alt+a to the glyph 'å' (no modifier) — decoded by the
        // shared UTF-8 path, unchanged from baseline.
        assert_eq!(enh(&[0xc3, 0xa5]), key(Key::Char('å'), Modifiers::NONE, 2));
        // Alacritty sends alt+a as `ESC [ 97 ; 3 u` (mod 3 = 1+alt) — a real
        // alt+a. The two-camp split persists into enhanced mode.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x33, 0x75]),
            key(Key::Char('a'), Modifiers::ALT, 7)
        );
    }

    // --- Boundary / abnormal CSI-u inputs the clean fixtures never exercise.
    //     These pin decoder-internal robustness against untrusted bytes, not
    //     terminal behaviour. ---

    #[test]
    fn enh_malformed_csi_u_is_unrecognized() {
        assert_eq!(enh(&[0x1b, 0x5b, b'u']), Decoded::Unrecognized); // no codepoint
        assert_eq!(enh(&[0x1b, 0x5b, b';', b'5', b'u']), Decoded::Unrecognized); // empty codepoint
        assert_eq!(
            enh(&[0x1b, 0x5b, b'9', b'7', b';', b'u']),
            Decoded::Unrecognized
        ); // empty mods
        assert_eq!(enh(&[0x1b, 0x5b, b'9', b'x', b'u']), Decoded::Unrecognized); // non-digit
        // A third `;`-separated field is an unmeasured richer CSI-u sub-grammar.
        assert_eq!(
            enh(&[0x1b, 0x5b, b'9', b'7', b';', b'5', b';', b'1', b'u']),
            Decoded::Unrecognized
        );
    }

    #[test]
    fn enh_csi_u_modifier_zero_is_rejected_not_underflowed() {
        // The modifier is 1 + bitfield, so 0 is illegal; it must not underflow.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x30, 0x75]),
            Decoded::Unrecognized
        );
    }

    #[test]
    fn enh_csi_u_codepoint_must_be_a_unicode_scalar() {
        // 0x110000 (one past char::MAX) and a surrogate 0xD800 are not scalars.
        assert_eq!(
            enh(b"\x1b[1114112u"), // 0x110000
            Decoded::Unrecognized
        );
        assert_eq!(
            enh(b"\x1b[55296u"), // 0xD800, a surrogate
            Decoded::Unrecognized
        );
    }

    #[test]
    fn enh_csi_u_huge_digit_run_overflows_to_unrecognized_without_panic() {
        assert_eq!(enh(b"\x1b[99999999999999999999;5u"), Decoded::Unrecognized);
    }

    #[test]
    fn enh_unknown_high_modifier_bits_are_ignored_not_rejected() {
        // mod value 21 = 1 + 0b10100 → ctrl (4) + an unmodelled bit (16, hyper).
        // The unknown bit is dropped; the chord we can represent survives.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x32, 0x31, 0x75]),
            key(Key::Char('a'), Modifiers::CTRL, 8)
        );
    }

    #[test]
    fn enh_truncated_csi_u_is_incomplete() {
        // A legal param run with no final `u` yet: read more.
        assert_eq!(
            enh(&[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x35]),
            Decoded::Incomplete
        );
    }

    #[test]
    fn enh_csi_u_with_intermediate_byte_is_unrecognized() {
        // 0x20 is a legal CSI intermediate, so `scan_csi` accepts it into the
        // params; the CSI-u decimal parser then rejects it (not a digit/`;`).
        assert_eq!(
            enh(&[0x1b, 0x5b, b'9', b'7', 0x20, b'u']),
            Decoded::Unrecognized
        );
    }

    // --- Cross-mode: the reason `DecodeMode` exists. ---

    #[test]
    fn same_bytes_decode_differently_per_mode() {
        // The enhanced ctrl+a sequence is a parameterised CSI: baseline rejects
        // it, enhanced reads it. Holding the bytes constant and varying only the
        // mode flips the verdict — the whole point of DecodeMode.
        let csi_u = &[0x1b, 0x5b, 0x39, 0x37, 0x3b, 0x35, 0x75];
        assert_eq!(base(csi_u), Decoded::Unrecognized);
        assert_eq!(enh(csi_u), key(Key::Char('a'), Modifiers::CTRL, 7));
        // A shape both modes share decodes identically.
        let up = &[0x1b, 0x5b, b'A'];
        assert_eq!(base(up), enh(up));
        // `0x09` still means Tab in BOTH modes (enhanced ctrl+i moved to CSI-u).
        assert_eq!(base(&[0x09]), enh(&[0x09]));
    }

    #[test]
    fn enh_never_panics_on_pathological_csi_u() {
        // Crafted shapes the recorded bytes never cover; assert only that the
        // decoder returns (never panics) on untrusted numeric garbage.
        let mut flood = vec![0x1b, 0x5b];
        flood.extend(std::iter::repeat_n(b'9', 500));
        flood.push(b'u');
        let _ = enh(&flood);
        let _ = enh(&[0x1b, 0x5b, b';', b';', b';', b'u']);
        let _ = enh(&[0x1b; 1000]);
    }
}
