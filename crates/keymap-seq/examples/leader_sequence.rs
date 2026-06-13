//! A leader / prefix-chord keymap driven by a simulated key stream.
//!
//! Shows the intended split: `keymap-seq` answers "exact / prefix / miss" for the
//! keys so far, and the *caller* owns the pending buffer and decides when to fire
//! or reset. The second demo (`timed`) uses [`TimedPending`] to add the time
//! dimension a `jj`-style binding needs — the library holds no clock, so `now` is
//! caller-supplied data.
//! Run with `cargo run -p keymap-seq --example leader_sequence`.

use std::time::{Duration, Instant};

use keymap_core::{Key, KeyInput, Modifiers};
use keymap_seq::{Match, SequenceKeymap, Step, TimedPending};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Save,
    Quit,
    GotoTop,
    NormalMode,
}

fn ctrl(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::CTRL)
}

fn plain(c: char) -> KeyInput {
    KeyInput::new(Key::Char(c), Modifiers::NONE)
}

fn main() {
    untimed();
    println!();
    timed();
}

/// Resolution without a clock: the buffer resets only on `Exact`/`NoMatch`.
fn untimed() {
    println!("== untimed ==");
    let mut map = SequenceKeymap::new();
    map.bind([ctrl('x'), ctrl('s')], Action::Save).unwrap();
    map.bind([ctrl('x'), ctrl('c')], Action::Quit).unwrap();
    map.bind([plain('g'), plain('g')], Action::GotoTop).unwrap();

    // A stream a terminal might deliver: a completed save, an abandoned prefix
    // (`ctrl+x` then an unrelated key), then `g g`.
    let stream = [
        ctrl('x'),
        ctrl('s'),
        ctrl('x'),
        plain('z'),
        plain('g'),
        plain('g'),
    ];

    let mut pending: Vec<KeyInput> = Vec::new();
    for key in stream {
        pending.push(key);
        match map.lookup(&pending) {
            Match::Exact(action) => {
                println!("{} -> fire {action:?}", render(&pending));
                pending.clear();
            }
            Match::Prefix => {
                println!("{} -> prefix, waiting", render(&pending));
            }
            Match::NoMatch => {
                println!("{} -> no binding, passing through", render(&pending));
                pending.clear();
            }
        }
    }
}

/// `jj`-style time window using [`TimedPending`].
///
/// [`TimedPending`] bundles the expiry check, flush, and new-key processing
/// into a single `feed` call. The library still holds no clock — `now` is
/// caller-supplied data (a `base + offset` pair here, so the demo is
/// deterministic and never flaky).
///
/// Note what this demo does *not* do: real vim `jj` also binds `j` on its own
/// (a literal cursor move). That needs both `j` *and* `[j, j]` bound — which
/// the prefix-free invariant forbids (`bind` rejects it with `PrefixShadow`).
/// Here `j` alone is unbound, so the timeout is the *only* caller policy needed.
fn timed() {
    const WINDOW: Duration = Duration::from_millis(500);

    println!("== timed (jj, via TimedPending) ==");
    let mut map = SequenceKeymap::new();
    map.bind([plain('j'), plain('j')], Action::NormalMode)
        .unwrap();

    // Deterministic timestamps: base + offset (no real sleep / Instant::now).
    let base = Instant::now();
    let stream = [
        (plain('j'), 0u64),
        (plain('j'), 120), // 120ms gap: quick, completes `jj`
        (plain('j'), 900),
        (plain('j'), 1700), // 800ms gap: too slow, expiry fires
    ];

    let mut pending = TimedPending::new();
    for (key, ms) in stream {
        let now = base + Duration::from_millis(ms);
        let result = pending.feed(&map, key, now, WINDOW);

        // If a previous prefix expired, report its keys as literals first.
        if let Some(expired) = result.expired {
            println!(
                "{} @ +{ms}ms -> idle timeout, flushed as literals",
                render(&expired),
            );
        }

        match result.step {
            Step::Fired(action) => println!("{key} @ +{ms}ms -> fire {action:?}"),
            Step::Pending => println!("{key} @ +{ms}ms -> prefix, waiting (window {WINDOW:?})"),
            Step::PassThrough(keys) => {
                println!("{} @ +{ms}ms -> no binding, passing through", render(&keys));
            }
        }
    }

    // The stream ended mid-prefix. Drain as literals (what an idle timer does).
    let dangling = pending.flush();
    if !dangling.is_empty() {
        println!(
            "{} -> pending at end; flushed as literals",
            render(&dangling)
        );
    }
}

fn render(keys: &[KeyInput]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}
