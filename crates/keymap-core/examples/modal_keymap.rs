//! Context-dependent bindings via layered resolution.
//!
//! The same `ctrl+s` resolves to different actions depending on the app's
//! context — but the library never learns what a "context" is. The app holds
//! its own mode/focus state, decides which keymap layers are active and in what
//! priority order, and passes them to `resolve_layered`. Earlier layers win;
//! anything they don't bind falls through to the base layer.
//!
//! This *is* a lexical scope chain: `block → panel → global` resolved
//! innermost-first, first hit wins, a miss falls outward — the same shape as
//! JavaScript variable resolution. A miss in every layer (`None`) is the
//! "pass it through" signal; in a terminal multiplexer that is where a key flows
//! to the PTY, which sits *past the end of the chain* (a sink), never as a layer
//! inside it. See `keymap-tui` for a live, layer-by-layer view of this.
//!
//! Run with: `cargo run -p keymap-core --example modal_keymap`

use keymap_core::{Key, KeyInput, Keymap, Modifiers, resolve_layered};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Save,
    SplitPanel,
    Quit,
}

/// The app's own context — the library knows nothing about this type.
enum Context {
    Editor,
    Panel,
}

fn ctrl(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::CTRL)
}

fn main() {
    // Base layer: shared bindings live here once.
    let mut base = Keymap::new();
    base.bind(ctrl('s'), Action::Save);
    base.bind(ctrl('q'), Action::Quit);

    // Panel layer: only the overrides for panel context.
    let mut panel = Keymap::new();
    panel.bind(ctrl('s'), Action::SplitPanel);

    // The app maps its context to an ordered layer stack. This `match` is the
    // only place "context" exists — entirely on the application side.
    let layers_for = |ctx: &Context| -> Vec<&Keymap<Action>> {
        match ctx {
            Context::Editor => vec![&base],
            Context::Panel => vec![&panel, &base],
        }
    };

    for ctx in [Context::Editor, Context::Panel] {
        let name = match ctx {
            Context::Editor => "editor",
            Context::Panel => "panel",
        };
        let layers = layers_for(&ctx);
        for key in [ctrl('s'), ctrl('q')] {
            match resolve_layered(layers.iter().copied(), &key) {
                Some(action) => println!("[{name:>6}] {key:>6}  ->  {action:?}"),
                None => println!("[{name:>6}] {key:>6}  ->  pass through"),
            }
        }
    }
}
