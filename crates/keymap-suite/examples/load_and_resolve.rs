//! Loads a TOML keymap with one named layer plus a multi-key sequence,
//! prints what came back, then runs a short canned key stream through
//! both `resolve_layered` and a `PendingSequence` to show the suite's
//! one-import surface end to end.
//!
//! Run with: `cargo run -p keymap-suite --example load_and_resolve`

use keymap_suite::prelude::*;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Quit,
    Save,
    SplitPane,
}

fn resolve(name: &str) -> Option<Action> {
    match name {
        "quit" => Some(Action::Quit),
        "save" => Some(Action::Save),
        "split_pane" => Some(Action::SplitPane),
        _ => None,
    }
}

fn ctrl(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::CTRL)
}

fn main() {
    let toml = r#"
[keys]
"ctrl+q" = "quit"
"ctrl+s" = "save"
"ctrl+z" = "undo"            # unknown action — surfaces as a warning, not a failure

[layers.panel]
"ctrl+s" = "split_pane"      # overrides global ctrl+s when the panel layer is active

[[sequences]]
keys = ["ctrl+x", "ctrl+s"]
action = "save"
"#;

    let loaded = keymap_suite::from_toml_str(toml, resolve).expect("valid TOML and key strings");

    println!("layers: {:?}", loaded.layers.keys().collect::<Vec<_>>());
    println!("sequences bound: {}", !loaded.sequences.is_empty());
    if !loaded.warnings.is_empty() {
        println!("\n{} warning(s):", loaded.warnings.len());
        for w in &loaded.warnings {
            match w {
                Warning::UnknownAction { key, action } => {
                    println!("  {key}: unknown action {action:?}");
                }
                // The other variants are non-exhaustive; show that we handle them.
                other => println!("  {other:?}"),
            }
        }
    }

    // --- A short canned key stream demonstrating both resolve paths. ---

    // First with only the global layer active: ctrl+s = save.
    let chain_editor = [loaded.global()];
    let s = ctrl('s');
    println!(
        "\neditor context: ctrl+s -> {:?}",
        resolve_layered(chain_editor.iter().copied(), &s),
    );

    // Now the panel layer is also active and wins: ctrl+s = split_pane.
    let panel = &loaded.layers["panel"];
    let chain_panel = [panel, loaded.global()];
    println!(
        "panel context:  ctrl+s -> {:?}",
        resolve_layered(chain_panel.iter().copied(), &s),
    );

    // ctrl+q falls through the panel layer because it is only bound globally.
    let q = ctrl('q');
    println!(
        "panel context:  ctrl+q -> {:?}",
        resolve_layered(chain_panel.iter().copied(), &q),
    );

    // The sequence buffer: ctrl+x is a prefix, ctrl+s completes the binding.
    let mut pending = loaded.pending_sequence();
    println!(
        "\nsequence step 1: feed ctrl+x -> {:?}",
        pending.feed(&loaded.sequences, ctrl('x')),
    );
    println!(
        "sequence step 2: feed ctrl+s -> {:?}",
        pending.feed(&loaded.sequences, ctrl('s')),
    );

    // Discovery (the reverse of resolution): which keys run an action? This is
    // what a help screen or which-key menu renders. Order is unspecified, so we
    // format and sort for display.
    let mut save_keys: Vec<String> = keymap_suite::keys_for_action(loaded.global(), &Action::Save)
        .iter()
        .map(ToString::to_string)
        .collect();
    save_keys.sort();
    println!("\nhelp: 'save' is bound to {save_keys:?} in the global layer");
}
