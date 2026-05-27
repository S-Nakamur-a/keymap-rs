//! List bindings and search them. The library exposes the data (`iter()` plus
//! `KeyInput`'s `Display`); the fuzzy/search engine is your choice — here a tiny
//! case-insensitive substring filter.
//!
//! Run with: `cargo run -p keymap-core --example discovery`

use keymap_core::{Key, KeyInput, Keymap, Modifiers};

#[derive(Clone, Debug)]
struct Command {
    id: &'static str,
    description: &'static str,
}

fn main() {
    let mut keymap = Keymap::new();
    keymap.bind(
        KeyInput::new(Key::Char('q'), Modifiers::CTRL),
        Command {
            id: "quit",
            description: "Quit the application",
        },
    );
    keymap.bind(
        KeyInput::new(Key::Char('s'), Modifiers::CTRL),
        Command {
            id: "save",
            description: "Save the current file",
        },
    );
    keymap.bind(
        KeyInput::new(Key::F(1), Modifiers::NONE),
        Command {
            id: "help",
            description: "Show the help overlay",
        },
    );

    // Listing: collect and sort by the canonical key string.
    let mut listing: Vec<(String, &Command)> = keymap
        .iter()
        .map(|(input, cmd)| (input.to_string(), cmd))
        .collect();
    listing.sort_by(|a, b| a.0.cmp(&b.0));

    println!("all bindings:");
    for (key, cmd) in &listing {
        println!("  {key:>8}  {:<6} — {}", cmd.id, cmd.description);
    }

    search(&listing, "sav");
    search(&listing, "zzz");
}

fn search(listing: &[(String, &Command)], query: &str) {
    let needle = query.to_lowercase();
    let matches: Vec<&(String, &Command)> = listing
        .iter()
        .filter(|(key, cmd)| {
            let haystack = format!("{key} {} {}", cmd.id, cmd.description).to_lowercase();
            haystack.contains(&needle)
        })
        .collect();

    println!("\nsearch {query:?}:");
    if matches.is_empty() {
        // Don't leave the user guessing whether nothing matched or nothing exists.
        println!("  no bindings match {query:?}");
    } else {
        for (key, cmd) in matches {
            println!("  {key:>8}  {}", cmd.id);
        }
    }
}
