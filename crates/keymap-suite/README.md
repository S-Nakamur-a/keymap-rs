# keymap-suite

An opinionated facade over [`keymap-core`], [`keymap-config`], and [`keymap-seq`]: TOML in, action out, with the bookkeeping wired up.

For the nine out of ten Rust TUI authors who want their keymap to be:

- **configurable** from a TOML file (without writing the loader yourself),
- **layered** so the same chord can mean different things in editor / panel / popup context,
- **sequence-aware** so `ctrl+x ctrl+s` and leader keys do not need a hand-rolled buffer, and
- **discoverable** so a help screen or which-key menu can list "what runs this action?" ([`keys_for_action`]).

`keymap-suite` is the entry point. Authors who want to write their own backend, decode raw terminal bytes (`keymap-term`), or do empirical reachability work drop down to the individual crates.

## Add it to your project

```toml
[dependencies]
keymap-suite = "0.1"
# Reading events through crossterm? Turn on the adapter:
keymap-suite = { version = "0.1", features = ["crossterm"] }
```

With the `crossterm` feature, `KeyInput::try_from(key_event)` converts a
`crossterm::event::KeyEvent`; a key with no neutral form returns the
re-exported `UnsupportedKey`. The default build stays backend-neutral.

## A small modal TUI in 30-ish lines

```rust
use std::time::Duration;
use keymap_suite::prelude::*;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Quit,
    Save,
    SplitPane,
}

fn resolve(name: &str) -> Option<Action> {
    match name {
        "quit"       => Some(Action::Quit),
        "save"       => Some(Action::Save),
        "split_pane" => Some(Action::SplitPane),
        _            => None,
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let loaded = keymap_suite::from_toml_path("keys.toml", resolve)?;

    // The active layer chain is *yours* — picked from your application state.
    let panel_active = false;
    let chain: Vec<&Keymap<Action>> = if panel_active {
        vec![&loaded.layers["panel"], loaded.global()]
    } else {
        vec![loaded.global()]
    };

    // Multi-key sequence buffer is yours to own as a field.
    let mut pending = loaded.pending_sequence();

    loop {
        let key = read_key_with_timeout(Duration::from_millis(500))?;

        // Single-chord first; on a miss, try the sequence buffer.
        if let Some(action) = resolve_layered(chain.iter().copied(), &key) {
            apply(action);
            continue;
        }
        match pending.feed(&loaded.sequences, key) {
            Step::Fired(action)         => apply(action),
            Step::Pending               => { /* restart idle timer */ }
            Step::PassThrough(literals) => for k in literals { handle_literal(k) },
        }
    }
}
# fn read_key_with_timeout(_: Duration) -> Result<KeyInput, Box<dyn std::error::Error>> { unimplemented!() }
# fn apply(_: &Action) {}
# fn handle_literal(_: KeyInput) {}
```

Companion `keys.toml`:

```toml
[keys]
"ctrl+q" = "quit"
"ctrl+s" = "save"

[layers.panel]
"ctrl+s" = "split_pane"   # overrides global ctrl+s when the panel is active

[[sequences]]
keys = ["ctrl+x", "ctrl+s"]
action = "save"
```

## Lenient by default, opt into strict

The loader collects non-fatal problems (chord conflicts within a layer, unknown action names, sequence prefix shadows, …) into `loaded.warnings` rather than failing. That keeps the same TOML behaving identically whether you load it via this suite or via `keymap-config` directly — so a user-edited dotfile with a typo does not crash the TUI on startup.

When you do want a strict gate (CI, production startup, behind a flag) one call expresses it:

```rust
use keymap_suite::prelude::*;

# #[derive(Clone, Debug, PartialEq)] enum Action { Save }
# let toml = r#""#;
let loaded = keymap_suite::from_toml_str(toml, |_| None::<Action>)?
    .deny_warnings()
    .map_err(|warnings| format!("{} keymap warning(s); aborting", warnings.len()))?;
# let _ = loaded;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Discovery: a help screen, in reverse

Resolution answers "what does this key do?"; a help screen or which-key menu asks the reverse, "what keys run this action?". `keys_for_action` is that reverse lookup over one layer:

```rust
use keymap_suite::prelude::*;

# #[derive(Clone, Debug, PartialEq)] enum Action { Save }
# let mut map = Keymap::new();
# map.bind(KeyInput::new(Key::Char('s'), Modifiers::CTRL), Action::Save);
let mut keys: Vec<String> = keys_for_action(&map, &Action::Save)
    .iter()
    .map(ToString::to_string) // chords come back borrowed and unordered …
    .collect();
keys.sort();                  // … so you format and sort for display
```

It works on one layer; for a layered chain, map it over your active layers (the suite does not fold the chain, because which layers are active is your state).

## What state lives where

| Belongs to the suite | Belongs to you (the caller) |
| --- | --- |
| The loaded keymap tables (`Loaded::layers`, `Loaded::sequences`) | The active layer chain for the current event |
| `PendingSequence` — the multi-key buffer | The inter-key timer that calls `pending.flush()` when a prefix is abandoned |
| Lenient warnings | The decision to `deny_warnings()` (CI), log, or ignore |

The library never holds a clock or a mode. That is `keymap-rs`'s spine — see [`docs/STATUS.md`](../../docs/STATUS.md) and the workspace [`README`](../../README.md) for the rationale.

## When to drop a level lower

| If you need to … | Reach for |
| --- | --- |
| Decode raw terminal bytes back to a `KeyInput` (PTY host, multiplexer) | [`keymap-term`] |
| Lint a keymap for chords a legacy C0 terminal cannot deliver | `keymap_core::legacy_lints` |
| Map an enum to/from config action names without hand-writing `from_str` | [`strum`](https://crates.io/crates/strum) on your `Action` enum |
| Manage your own re-export discipline | the four foundation crates directly |

[`keymap-core`]: https://crates.io/crates/keymap-core
[`keymap-config`]: https://crates.io/crates/keymap-config
[`keymap-seq`]: https://crates.io/crates/keymap-seq
[`keymap-term`]: https://crates.io/crates/keymap-term
[`keys_for_action`]: https://docs.rs/keymap-suite/latest/keymap_suite/fn.keys_for_action.html
