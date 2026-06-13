//! Command palette with front-prefix completion.
//!
//! This demo shows [`CommandIndex`] in the role it is designed for: a caller
//! types a partial command name, the index enumerates matching candidates in
//! lexicographic order, and the caller dispatches the chosen (or the exact)
//! command.
//!
//! **What [`CommandIndex`] owns:**
//! - Binding name → action (`bind`).
//! - Exact lookup (`get`).
//! - Prefix enumeration in lexicographic order (`complete`).
//! - Full listing for a palette menu (`iter`).
//!
//! **What stays with the caller:**
//! - Case normalization and trimming — the index stores names byte-for-byte.
//! - The line-editing buffer that accumulates what the user types.
//! - Selecting among multiple completions and actual dispatch.
//!
//! For the *execution* shape of an ex-command (`:` key → line buffer → Enter →
//! `FnMut(&str) -> Option<A>`) see `examples/ex_command.rs`.
//!
//! Run with: `cargo run -p keymap-core --example command_palette`

use keymap_core::cmd::CommandIndex;

#[derive(Debug, PartialEq)]
enum Action {
    Write,
    WriteQuit,
    WriteQuitAll,
    Quit,
    QuitAll,
}

fn main() {
    let mut idx = CommandIndex::new();
    idx.bind("write", Action::Write);
    idx.bind("write-quit", Action::WriteQuit);
    idx.bind("write-quit-all", Action::WriteQuitAll);
    idx.bind("quit", Action::Quit);
    idx.bind("quit-all", Action::QuitAll);

    // --- Full palette listing (e.g. user opens the palette with no text yet) ---
    println!("All commands:");
    for (name, action) in idx.iter() {
        println!("  {name:20} -> {action:?}");
    }

    // --- Prefix completion: user has typed "w" ---
    println!("\nCompletions for \"w\":");
    for (name, action) in idx.complete("w") {
        println!("  {name:20} -> {action:?}");
    }

    // --- Prefix completion: user has typed "write-q" ---
    println!("\nCompletions for \"write-q\":");
    for (name, action) in idx.complete("write-q") {
        println!("  {name:20} -> {action:?}");
    }

    // --- Exact lookup after the user presses Enter on "write" ---
    println!("\nExact lookup:");
    match idx.get("write") {
        Some(action) => println!("  \"write\" -> {action:?}"),
        None => println!("  \"write\" -> unknown"),
    }

    // --- Case normalization is the caller's responsibility ---
    //
    // The index stores names byte-for-byte. If the user typed "Write" (capital
    // W), the caller normalizes to lowercase before calling get/complete:
    let user_input = "Write";
    let normalized = user_input.to_lowercase();
    println!("\nUser typed {user_input:?}, normalized to {normalized:?}:");
    match idx.get(&normalized) {
        Some(action) => println!("  -> {action:?}"),
        None => println!("  -> unknown"),
    }

    // --- Non-exclusive: ":w" can be both exact and a prefix ---
    //
    // In a vim-style command line `:w` executes "write" and `:wq` executes
    // "write-quit". Both can coexist because get and complete are orthogonal:
    // get(":w") is exact, complete(":w") enumerates ":w" and ":wq" and ":wqa".
    let mut vim = CommandIndex::new();
    vim.bind(":w", "write");
    vim.bind(":wq", "write-quit");
    vim.bind(":wqa", "write-quit-all");

    println!("\nVim-style coexistence:");
    println!("  get(\":w\") = {:?}", vim.get(":w"));
    let completions: Vec<&str> = vim.complete(":w").map(|(n, _)| n).collect();
    println!("  complete(\":w\") = {completions:?}");
}
