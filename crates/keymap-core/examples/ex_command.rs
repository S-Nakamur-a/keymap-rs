//! Ex-command (`:wq`) execution, composed entirely caller-side.
//!
//! A command line like vim's `:wq` is **not** a chord sequence, so it does not
//! use `keymap-seq` (that crate is a prefix-free chord trie; a command line is an
//! open string language — `:w`, `:wq`, `:wqa` coexist, and editing keys mutate a
//! buffer rather than extend a sequence). The *execution* path decomposes into
//! three stages, and the library adds no new type for them:
//!
//! 1. **Press `:`** — an ordinary single-key binding. `Keymap::get` returns
//!    `EnterCommandMode`, exactly like any other action.
//! 2. **Capture `wq`** — the typed text accumulates into a caller-owned line
//!    buffer. This is caller state, the same way `keymap-seq`'s pending buffer is.
//! 3. **Dispatch on Enter** — the buffer is resolved to an action by a
//!    `FnMut(&str) -> Option<Action>` closure. That is the *same shape* as
//!    `keymap_config::from_str`'s name resolver; we reuse the shape, not that
//!    crate's type, so the execution path remains a plain closure.
//!
//! **Execution vs discovery are orthogonal.** This example shows the execution
//! path. For the *discovery* layer — front-prefix completion (`:w<Tab>`) and a
//! full palette listing — see `examples/command_palette.rs` and
//! [`keymap_core::cmd::CommandIndex`]. The two are independent: you can use one
//! without the other, and combining them is a matter of wiring in the caller.
//!
//! Deliberate non-goals: compound splitting (`wq` → `w` + `q`), arguments
//! (`:w file`), ranges (`:%s/a/b/g`). See `docs/ROADMAP.md`.
//!
//! Run with: `cargo run -p keymap-core --example ex_command`

use keymap_core::{Key, KeyInput, Keymap, Modifiers};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    /// Bound to `:` in the keymap — opens the command line.
    EnterCommandMode,
    /// Resolved from the command name on Enter.
    Write,
    Quit,
    WriteQuit,
}

/// The caller's own command-line state. The library never sees this type; the
/// line buffer lives *inside* the `Command` variant, so a buffer can't exist
/// while we're in `Normal` mode (illegal states are unrepresentable).
enum Mode {
    Normal,
    Command(String),
}

fn plain(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::NONE)
}

fn enter() -> KeyInput {
    KeyInput::new(Key::Enter, Modifiers::NONE)
}

fn main() {
    let mut map = Keymap::new();
    map.bind(plain(':'), Action::EnterCommandMode);

    // Stage 3's resolver: command name -> action. Same shape as
    // `keymap_config`'s `FnMut(&str) -> Option<A>`, intentionally a plain
    // closure rather than that crate's type.
    let resolve = |name: &str| -> Option<Action> {
        match name {
            "w" => Some(Action::Write),
            "q" => Some(Action::Quit),
            "wq" => Some(Action::WriteQuit),
            _ => None,
        }
    };

    // A simulated keystroke stream: `:wq⏎`, then `:q⏎`, then an unknown `:zz⏎`.
    let stream = [
        plain(':'),
        plain('w'),
        plain('q'),
        enter(),
        plain(':'),
        plain('q'),
        enter(),
        plain(':'),
        plain('z'),
        plain('z'),
        enter(),
    ];

    let mut mode = Mode::Normal;
    for key in stream {
        match &mut mode {
            Mode::Normal => {
                if map.get(&key) == Some(&Action::EnterCommandMode) {
                    println!(": -> enter command mode");
                    mode = Mode::Command(String::new());
                } else {
                    // Any other normal-mode key would run its own binding; this
                    // demo only cares about the `:` entry point.
                }
            }
            Mode::Command(buf) => match key.key() {
                Key::Char(c) => {
                    buf.push(c);
                    println!("  :{buf}");
                }
                Key::Enter => {
                    // Take the buffer out and drop back to Normal in one move.
                    let name = std::mem::take(buf);
                    match resolve(&name) {
                        Some(action) => println!("  :{name} -> fire {action:?}"),
                        None => println!("  :{name} -> unknown command, no-op"),
                    }
                    mode = Mode::Normal;
                }
                Key::Esc => {
                    println!("  esc -> abandon command line");
                    mode = Mode::Normal;
                }
                // `Key` is #[non_exhaustive]; other editing keys are ignored here.
                _ => {}
            },
        }
    }
}
