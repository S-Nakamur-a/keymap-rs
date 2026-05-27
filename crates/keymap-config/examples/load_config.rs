//! Load bindings from TOML — across named layers — resolve action names, and
//! surface the problems that shouldn't pass silently: a chord conflict and an
//! unknown action.
//!
//! The bare `[keys]` table is the implicit `global` layer; each `[layers.<name>]`
//! table is a layer the caller activates when it likes. Note that `ctrl+s` is
//! bound in *both* `global` and `panel`: that is an override the caller composes
//! with `resolve_layered`, not a conflict, so it draws no warning.
//!
//! Run with: `cargo run -p keymap-config --example load_config`

use keymap_config::Warning;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Quit,
    Save,
    SplitPane,
}

fn main() {
    let toml = r#"
[keys]
"ctrl+q" = "quit"
"ctrl+s" = "save"
"control+s" = "split_pane"   # same chord as ctrl+s -> conflict (within global)
"ctrl+z" = "undo"            # no such action -> unknown

[layers.panel]
"ctrl+s" = "split_pane"      # overrides global's ctrl+s when panel is active
"#;

    let out = keymap_config::from_str(toml, |name| match name {
        "quit" => Some(Action::Quit),
        "save" => Some(Action::Save),
        "split_pane" => Some(Action::SplitPane),
        _ => None,
    })
    .expect("valid TOML and key strings");

    for (name, keymap) in &out.layers {
        println!("layer {name:?} ({} binding(s)):", keymap.len());
        let mut listing: Vec<(String, &Action)> = keymap
            .iter()
            .map(|(input, action)| (input.to_string(), action))
            .collect();
        listing.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, action) in &listing {
            println!("  {key:>8}  ->  {action:?}");
        }
    }

    if !out.warnings.is_empty() {
        println!("\nwarnings:");
        for warning in &out.warnings {
            match warning {
                Warning::Conflict {
                    chord,
                    contenders,
                    winner,
                } => {
                    println!("  conflict on {chord}: {contenders:?} — kept {winner:?}");
                }
                Warning::UnknownAction { key, action } => {
                    println!("  {key}: unknown action {action:?}");
                }
                // `Warning` is #[non_exhaustive]; handle future variants.
                _ => println!("  (other warning)"),
            }
        }
    }
}
