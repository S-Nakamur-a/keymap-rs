//! Bind some keys, then resolve inputs. A miss (`None`) is the "pass it through"
//! signal for the same-process case.
//!
//! Run with: `cargo run -p keymap-core --example basic_lookup`

use keymap_core::{Key, KeyInput, Keymap, Modifiers};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Quit,
    Save,
    SplitPane,
}

fn main() {
    let mut keymap = Keymap::new();
    keymap.bind(KeyInput::new(Key::Char('q'), Modifiers::CTRL), Action::Quit);
    keymap.bind(KeyInput::new(Key::Char('s'), Modifiers::CTRL), Action::Save);
    keymap.bind(
        KeyInput::new(Key::Char('1'), Modifiers::SUPER),
        Action::SplitPane,
    );

    // In a real app these come from the terminal backend (see the `crossterm`
    // feature's `TryFrom<KeyEvent>`); here we construct them directly.
    let inputs = [
        KeyInput::new(Key::Char('q'), Modifiers::CTRL),
        KeyInput::new(Key::Char('1'), Modifiers::SUPER),
        KeyInput::new(Key::Char('x'), Modifiers::NONE),
    ];

    for input in inputs {
        match keymap.get(&input) {
            Some(action) => println!("{input:>8}  ->  consume {action:?}"),
            None => println!("{input:>8}  ->  pass through"),
        }
    }
}
