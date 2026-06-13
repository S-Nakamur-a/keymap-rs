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
# keymap-suite = { version = "0.1", features = ["crossterm"] }
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

    // (In a real app: loop { let key = read_key(); ... })
    let key = "ctrl+s".parse::<KeyInput>()?;

    // Single-chord first; on a miss, try the sequence buffer.
    if let Some(action) = resolve_layered(chain.iter().copied(), &key) {
        println!("chord: {action:?}");
    }
    match pending.feed(&loaded.sequences, key) {
        Step::Fired(action)         => println!("seq: {action:?}"),
        Step::Pending               => { /* restart idle timer */ }
        Step::PassThrough(literals) => println!("{} key(s) passed through", literals.len()),
    }
    Ok(())
}
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
# Ok::<(), keymap_suite::BuildError>(())
```

It works on one layer; for a layered chain, map it over your active layers (the suite does not fold the chain, because which layers are active is your state).

## Building chords in code

Most chords come from your TOML file or arrive as a `KeyInput` at runtime, so you rarely construct one by hand. When you do — tests, or a few hard-coded defaults — the `chords` module keeps it terse:

```rust
use keymap_suite::chords::{ctrl, key};

assert_eq!(ctrl('s'), "ctrl+s".parse().unwrap());
assert_eq!(key('q'),  "q".parse().unwrap());
```

`key` / `ctrl` / `alt` cover the modifier-plus-character case and normalize the same way the loader does, so a chord you build matches one that arrives at runtime. For named keys (`Tab`, `F1`) or multi-modifier chords (`ctrl+shift+s`), parse the canonical string — `"ctrl+shift+f1".parse::<KeyInput>()` — which handles them all uniformly. (There is no `shift('a')`: a sole Shift on a character folds away, so it would just equal `key('a')`.)

## New in 0.1: five more capabilities

All five are re-exported from the suite root (not the prelude — more specialised).

**`CommandIndex<A>`** — `bind(name, action)`, `get(name) -> Option<&A>` for exact dispatch, `complete(prefix) -> impl Iterator<Item = (&str, &A)>` for prefix-completion as the user types. No fuzzy matching, no extra crate.

**`TimedPending`** — wraps `PendingSequence` and adds `deadline(window) -> Option<Instant>` so your event-loop poll timeout derives directly from the last key's timestamp. Feed it `(map, key, now, window)`.

**`validate_rebind(&layers, target, proposed, &reserved) -> RebindVerdict`** — checks whether a proposed chord collides with reserved keys (including legacy-terminal collisions) before any live keymap mutation.

**`merge(base, overlay) -> Merged<A>`** — layers a user TOML over compiled-in defaults. In the overlay, `"ctrl+s" = false` is a tombstone. `to_toml_layered_with_unbinds` round-trips the tombstones. Override notes land in `Merged::notes`, not `Warning`, so `.deny_warnings()` is unaffected.

**`Warning::kind() -> WarningKind`** — match on `Conflict | UnknownAction | PrefixShadow | EmptySequence | SequenceShadow` without destructuring. `impl Display for Warning` gives a one-line human-readable string.

See [`docs/SHOWCASE.md`](../../docs/SHOWCASE.md) for the first real consumer that wires all five values, with measured before/after glue-line counts (after ≈ 87 lines vs before ≈ 160+).

## Warning category matching

`Warning::kind()` returns a `WarningKind` so you can categorise warnings without destructuring each variant:

```rust
use keymap_suite::{Warning, WarningKind};

# let toml = "[keys]\n\"ctrl+s\" = \"unknown\"\n";
# #[derive(Clone, Debug, PartialEq)] enum Action {}
let loaded = keymap_suite::from_toml_str(toml, |_| None::<Action>)
    .expect("valid TOML");
for w in &loaded.warnings {
    match w.kind() {
        WarningKind::UnknownAction => eprintln!("unknown action: {w}"),
        WarningKind::Conflict      => eprintln!("conflict: {w}"),
        _                          => {}
    }
}
```

## What state lives where

| Belongs to the suite | Belongs to you (the caller) |
| --- | --- |
| The loaded keymap tables (`Loaded::layers`, `Loaded::sequences`) | The active layer chain for the current event |
| `PendingSequence` / `TimedPending` — the multi-key buffer | The inter-key timer that calls `pending.flush()` when a prefix is abandoned |
| Lenient warnings | The decision to `deny_warnings()` (CI), log, or ignore |

`TimedPending::deadline(window)` returns the `Instant` by which the pending prefix must be resolved — use it as your event-loop poll timeout so the flush fires at the right moment. You still pass `now: Instant` in; the library reads no clock.

## Boilerplate tip: `strum` for your action enum

The `resolve` closure you write by hand can be replaced by [`strum`](https://crates.io/crates/strum) (`EnumString` + `AsRefStr` with `serialize_all = "snake_case"`), which derives `from_str` / `as_ref` for your action enum. `keymap-rs` itself does not depend on strum; it is your choice:

```toml
[dependencies]
strum = { version = "0.26", features = ["derive"] }
```

```rust
// With strum on your enum, the resolve closure becomes one line:
// |name| name.parse::<Action>().ok()
```

## When to drop a level lower

| If you need to … | Reach for |
| --- | --- |
| Decode raw terminal bytes back to a `KeyInput` (PTY host, multiplexer) | [`keymap-term`] |
| Lint a keymap for chords a legacy C0 terminal cannot deliver | `keymap_suite::legacy_lints` (now re-exported from suite) |
| Map an enum to/from config action names without hand-writing `from_str` | [`strum`](https://crates.io/crates/strum) on your `Action` enum |
| Manage your own re-export discipline | the four foundation crates directly |

The library never holds a clock or a mode. That is `keymap-rs`'s spine — see [`docs/STATUS.md`](../../docs/STATUS.md) and the workspace [`README`](../../README.md) for the rationale.

[`keymap-core`]: https://crates.io/crates/keymap-core
[`keymap-config`]: https://crates.io/crates/keymap-config
[`keymap-seq`]: https://crates.io/crates/keymap-seq
[`keymap-term`]: https://crates.io/crates/keymap-term
[`keys_for_action`]: https://docs.rs/keymap-suite/latest/keymap_suite/fn.keys_for_action.html
