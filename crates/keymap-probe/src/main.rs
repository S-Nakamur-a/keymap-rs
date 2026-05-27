//! `keymap-probe` — interactive recorder of what a real terminal sends.
//!
//! Run this inside each terminal you care about (optionally under tmux / over
//! SSH). It sends a capability query, asks you to press a fixed list of keys,
//! and records the raw bytes each one produced into a `captures/*.toml` file.
//!
//! Design constraints (see the Phase 1 discussion):
//! - The terminal's raw bytes are the primary datum, so we do **not** feed input
//!   through crossterm's event parser. crossterm is used only to toggle raw
//!   mode; readiness is checked with `rustix::event::poll` and bytes are read
//!   with `rustix::io::read`, so nothing buffers or reinterprets the stream.
//! - The recorder never claims a value it didn't observe: a key that produces no
//!   bytes within the timeout is recorded as empty (that *is* the finding).

use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use keymap_core::{KeyInput, Modifiers};
use keymap_term::{Capability, Capture, KeyPress, Meta, Provenance, to_hex};
use rustix::event::{PollFd, PollFlags, poll};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

type AnyError = Box<dyn std::error::Error>;

/// Keys to record, as human labels. Includes the painful cases: `cmd+1` (often
/// swallowed by the OS/terminal), `ctrl+i` (collides with Tab in legacy
/// encodings), and `shift+tab` (two encodings that must converge).
const KEYS_TO_RECORD: &[&str] = &[
    "ctrl+a",
    "cmd+1",
    "alt+a",
    "ctrl+i",
    "shift+tab",
    "f1",
    "up",
    "down",
    "left",
    "right",
    "esc",
];

/// How long to wait for the first byte of a keypress before recording "nothing
/// arrived" and moving on (milliseconds).
const FIRST_BYTE_TIMEOUT_MS: i32 = 6_000;
/// Idle gap that marks the end of one escape sequence (milliseconds).
const IDLE_GAP_MS: i32 = 80;

/// The recorded `mode` for a baseline capture (no progressive-enhancement flags).
const MODE_BASELINE: &str = "baseline";
/// The recorded `mode` for a capture taken with the kitty keyboard protocol
/// pushed (`DecodeMode::from_capture_mode` learns to read this in the enhanced
/// decode increment).
const MODE_KITTY_ENHANCED: &str = "kitty-enhanced";

fn main() -> Result<(), AnyError> {
    // The only flag: `--enhanced` records with the kitty keyboard protocol pushed
    // (the enhanced re-capture). Default is a baseline capture, unchanged.
    let enhanced = std::env::args().skip(1).any(|a| a == "--enhanced");

    let meta = gather_meta();
    print_banner(&meta, enhanced);

    let stdin = std::io::stdin();
    let fd = stdin.as_fd();

    let raw = RawGuard::enable().map_err(|e| {
        format!("could not enter raw mode ({e}); run keymap-probe in an interactive terminal")
    })?;

    let capabilities = probe_capabilities(fd)?;
    let kitty = capabilities
        .iter()
        .find(|c| c.name == "kitty_keyboard_protocol")
        .map_or("?", |c| c.value.as_str());
    say(&format!("kitty keyboard protocol: {kitty}"));

    // Push the enhancement flags *after* probing capabilities (the kitty/DA1
    // query must be answered first). The guard pops them on drop — including on
    // panic — so the terminal is never left in enhanced mode.
    let (mode, enhancement) = if enhanced {
        if kitty != "supported" {
            return Err(format!(
                "--enhanced needs the kitty keyboard protocol, but this terminal reports it \
                 \"{kitty}\"; drop --enhanced to record a baseline capture instead"
            )
            .into());
        }
        say("pushing kitty progressive-enhancement flags (DISAMBIGUATE_ESCAPE_CODES)…");
        (MODE_KITTY_ENHANCED, Some(EnhancementGuard::push()?))
    } else {
        (MODE_BASELINE, None)
    };
    say(&format!("recording mode: {mode}"));
    say("");

    let mut keypresses = Vec::new();
    for &intended in KEYS_TO_RECORD {
        say(&format!("press  [{intended}]  (ctrl+c to stop early)"));
        let (bytes, chunks) = read_until_idle(fd, FIRST_BYTE_TIMEOUT_MS, IDLE_GAP_MS)?;

        if bytes == [0x03] {
            say("ctrl+c — stopping.");
            break;
        }

        if bytes.is_empty() {
            say("  …no bytes within timeout (not delivered to the app)");
        } else {
            say(&format!("  -> {}", to_hex(&bytes).join(" ")));
        }

        // The key-string grammar lives in keymap-core; `.ok()` preserves the
        // "record nothing if unparseable" behavior.
        let (expected_key, expected_mods) = match intended.parse::<KeyInput>().ok() {
            Some(ki) => {
                let (k, m) = expected_strings(&ki);
                (Some(k), Some(m))
            }
            None => (None, None),
        };

        keypresses.push(KeyPress {
            intended: intended.to_string(),
            mode: mode.to_string(),
            raw_bytes: to_hex(&bytes),
            read_chunks: chunks,
            expected_key,
            expected_mods,
        });
    }

    // Pop the enhancement flags before restoring cooked mode.
    drop(enhancement);
    drop(raw);

    let capture = Capture {
        meta,
        capability: capabilities,
        keypress: keypresses,
    };
    let path = write_capture(&capture, mode)?;
    println!("wrote {path}");
    Ok(())
}

/// Restores cooked mode on drop, including on panic, so we never leave the
/// terminal wedged.
struct RawGuard;

impl RawGuard {
    fn enable() -> std::io::Result<Self> {
        enable_raw_mode()?;
        Ok(RawGuard)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Pushes the kitty progressive-enhancement flags on creation and pops them on
/// drop (including on panic), so the terminal is never left in enhanced mode.
/// Mirrors `keymap-tui`'s F3 toggle so both tools push the *same* flag — the
/// captures must reflect exactly what the inspector and decoder will later see.
struct EnhancementGuard;

impl EnhancementGuard {
    fn push() -> std::io::Result<Self> {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        crossterm::execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        Ok(EnhancementGuard)
    }
}

impl Drop for EnhancementGuard {
    fn drop(&mut self) {
        use crossterm::event::PopKeyboardEnhancementFlags;
        let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
}

fn gather_meta() -> Meta {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    Meta {
        captured_at: now_rfc3339(),
        harness_version: env!("CARGO_PKG_VERSION").to_string(),
        term: env("TERM"),
        term_program: env("TERM_PROGRAM"),
        term_program_version: env("TERM_PROGRAM_VERSION"),
        tmux: env("TMUX").is_some(),
        ssh: env("SSH_TTY").is_some() || env("SSH_CONNECTION").is_some(),
    }
}

fn print_banner(meta: &Meta, enhanced: bool) {
    let term = meta.term.as_deref().unwrap_or("?");
    let prog = meta.term_program.as_deref().unwrap_or("?");
    let mode = if enhanced {
        "kitty-enhanced (--enhanced)"
    } else {
        "baseline"
    };
    eprintln!("keymap-probe — recording what this terminal sends");
    eprintln!(
        "  TERM={term} TERM_PROGRAM={prog} tmux={} ssh={}  mode={mode}",
        meta.tmux, meta.ssh
    );
    eprintln!("  press each prompted key once; we record the raw bytes.");
    eprintln!();
}

/// Sends the kitty keyboard-protocol query followed by a DA1 query (the latter
/// is a sync barrier: every terminal answers it, so once its reply arrives, a
/// missing kitty reply means "unsupported", not "still in flight").
fn probe_capabilities(fd: BorrowedFd<'_>) -> Result<Vec<Capability>, AnyError> {
    // ESC [ ? u  (kitty query)   then   ESC [ c  (DA1)
    let query: &[u8] = b"\x1b[?u\x1b[c";
    let mut out = std::io::stdout();
    out.write_all(query)?;
    out.flush()?;

    let (response, _chunks) = read_until_idle(fd, 500, 100)?;
    let value = if detect_kitty_support(&response) {
        "supported"
    } else {
        "unsupported"
    };

    Ok(vec![Capability {
        name: "kitty_keyboard_protocol".to_string(),
        value: value.to_string(),
        provenance: Provenance::QueryResponse,
        raw_response: to_hex(&response),
    }])
}

/// Reads bytes until the stream goes idle for `idle_ms`, returning the bytes and
/// the size of each `read()` (preserving where reads split a sequence).
fn read_until_idle(
    fd: BorrowedFd<'_>,
    first_ms: i32,
    idle_ms: i32,
) -> Result<(Vec<u8>, Vec<usize>), AnyError> {
    let mut bytes = Vec::new();
    let mut chunks = Vec::new();

    if !poll_readable(fd, first_ms)? {
        return Ok((bytes, chunks));
    }
    loop {
        let mut buf = [0u8; 64];
        let n = rustix::io::read(fd, &mut buf)?;
        if n == 0 {
            break; // EOF
        }
        bytes.extend_from_slice(&buf[..n]);
        chunks.push(n);
        if !poll_readable(fd, idle_ms)? {
            break;
        }
    }
    Ok((bytes, chunks))
}

fn poll_readable(fd: BorrowedFd<'_>, timeout_ms: i32) -> Result<bool, AnyError> {
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    poll(&mut fds, timeout_ms)?;
    Ok(fds[0].revents().contains(PollFlags::IN))
}

/// True if the first `ESC [ ?` CSI response terminates in `u` (a kitty
/// progressive-enhancement reply) rather than `c` (which would be the DA1 reply,
/// meaning the kitty query was ignored).
fn detect_kitty_support(response: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 < response.len() {
        if response[i] == 0x1b && response[i + 1] == b'[' && response[i + 2] == b'?' {
            for &b in &response[i + 3..] {
                // A CSI final byte is in 0x40..=0x7e; params come before it.
                if (0x40..=0x7e).contains(&b) {
                    return b == b'u';
                }
            }
            return false;
        }
        i += 1;
    }
    false
}

fn expected_strings(input: &KeyInput) -> (String, Vec<String>) {
    (format!("{:?}", input.key()), mod_strings(input.modifiers()))
}

fn mod_strings(mods: Modifiers) -> Vec<String> {
    let mut out = Vec::new();
    for (flag, name) in [
        (Modifiers::CTRL, "CTRL"),
        (Modifiers::ALT, "ALT"),
        (Modifiers::SHIFT, "SHIFT"),
        (Modifiers::SUPER, "SUPER"),
    ] {
        if mods.contains(flag) {
            out.push(name.to_string());
        }
    }
    out
}

fn write_capture(capture: &Capture, mode: &str) -> Result<String, AnyError> {
    std::fs::create_dir_all("captures")?;
    let label = capture
        .meta
        .term_program
        .as_deref()
        .or(capture.meta.term.as_deref())
        .unwrap_or("unknown");
    let date = capture.meta.captured_at.get(..10).unwrap_or("unknown");
    let path = capture_path(label, mode, capture.meta.tmux, capture.meta.ssh, date);
    std::fs::write(&path, toml::to_string_pretty(capture)?)?;
    Ok(path)
}

/// Builds the capture filename. A baseline capture keeps the original scheme
/// (`{label}_tmux-…_ssh-…_{date}.toml`) so existing committed names stay stable;
/// a non-baseline mode adds a `{mode}` infix so it neither collides with the
/// baseline file nor hides which mode it holds.
fn capture_path(label: &str, mode: &str, tmux: bool, ssh: bool, date: &str) -> String {
    let mode_infix = if mode == MODE_BASELINE {
        String::new()
    } else {
        format!("{mode}_")
    };
    format!(
        "captures/{}_{}tmux-{}_ssh-{}_{}.toml",
        sanitize(label),
        mode_infix,
        if tmux { "on" } else { "off" },
        if ssh { "on" } else { "off" },
        date,
    )
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Prints a line in raw mode (`\r\n`, since the cursor won't auto-return).
fn say(msg: &str) {
    eprint!("{msg}\r\n");
    let _ = std::io::stderr().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_strings_match_capture_schema() {
        // The grammar itself is tested in keymap-core; here we only check the
        // capture-schema string shape the probe records.
        let input = "cmd+1".parse::<KeyInput>().unwrap();
        let (key, mods) = expected_strings(&input);
        assert_eq!(key, "Char('1')");
        assert_eq!(mods, vec!["SUPER".to_string()]);
    }

    #[test]
    fn baseline_capture_path_keeps_the_original_scheme() {
        // No mode infix for baseline, so existing committed filenames stay stable.
        assert_eq!(
            capture_path("iTerm-app", MODE_BASELINE, false, false, "2026-05-26"),
            "captures/iTerm-app_tmux-off_ssh-off_2026-05-26.toml"
        );
    }

    #[test]
    fn enhanced_capture_path_carries_the_mode_infix() {
        // The enhanced file is self-describing and cannot collide with baseline.
        assert_eq!(
            capture_path(
                "xterm-kitty",
                MODE_KITTY_ENHANCED,
                false,
                false,
                "2026-05-27"
            ),
            "captures/xterm-kitty_kitty-enhanced_tmux-off_ssh-off_2026-05-27.toml"
        );
    }

    #[test]
    fn detect_kitty_support_reads_final_byte() {
        // ESC [ ? 1 u  -> kitty reply (supported)
        assert!(detect_kitty_support(b"\x1b[?1u"));
        // ESC [ ? 62 ; 9 ; c  -> DA1 reply only (unsupported)
        assert!(!detect_kitty_support(b"\x1b[?62;9c"));
        // kitty reply arriving before the DA1 reply
        assert!(detect_kitty_support(b"\x1b[?5u\x1b[?62c"));
        assert!(!detect_kitty_support(b""));
    }
}
