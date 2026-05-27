//! A leader / prefix-chord keymap driven by a simulated key stream.
//!
//! Shows the intended split: `keymap-seq` answers "exact / prefix / miss" for the
//! keys so far, and the *caller* owns the pending buffer and decides when to fire
//! or reset. The second demo (`timed`) adds the time dimension a `jj`-style
//! binding needs — still entirely caller-side, because the library holds no clock.
//! Run with `cargo run -p keymap-seq --example leader_sequence`.

use std::time::Duration;

use keymap_core::{Key, KeyInput, Modifiers};
use keymap_seq::{Match, PendingSequence, SequenceKeymap, Step};

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

/// `jj`-style time window, driven through [`PendingSequence`]. The window —
/// "abandon a pending prefix if the next key is too slow" — is the *caller's*
/// policy, not the table's: `keymap-seq` has no clock, so the caller measures
/// inter-key time and decides when a prefix is stale. The helper owns the buffer
/// bookkeeping; the caller owns only the timing decision and the one `flush` call
/// it triggers.
///
/// Note what this demo does *not* do: real vim `jj` also binds `j` on its own (a
/// literal cursor move), so a lone `j` must fire while a quick `j j` escapes. That
/// needs both `j` *and* `[j, j]` bound — which the prefix-free invariant forbids
/// (`bind` would reject it with `PrefixShadow`). Resolving "is this `j` a literal
/// or the first half of `jj`?" is therefore caller timing policy layered on top
/// of `lookup`, not a trie outcome. Here `j` alone is unbound, so the timeout is
/// the *only* caller policy needed.
///
/// Unlike the raw-loop version this once was, the idle flush is *exercised*, not
/// just described: a too-slow key abandons the held prefix (mid-stream), and the
/// stream ends mid-prefix so the trailing `flush` drains the dangling `j` as a
/// literal — the case a real caller's idle timer fires on when no further key
/// arrives.
fn timed() {
    const WINDOW: Duration = Duration::from_millis(500);

    println!("== timed (jj) ==");
    let mut map = SequenceKeymap::new();
    map.bind([plain('j'), plain('j')], Action::NormalMode)
        .unwrap();

    // `(key, timestamp-since-start)`: a deterministic stand-in for an event
    // loop's clock (no real `Instant`/`sleep`, so the demo can't be flaky). We
    // compare the *inter-key* gap to the window — the gap since the last key the
    // pending prefix accepted, which is what "pressed twice quickly" means.
    let stream = [
        (plain('j'), Duration::from_millis(0)),
        (plain('j'), Duration::from_millis(120)), // quick: completes `jj`
        (plain('j'), Duration::from_millis(900)),
        (plain('j'), Duration::from_millis(1700)), // 800ms gap: too slow
    ];

    let mut pending = PendingSequence::new();
    let mut last: Option<Duration> = None;
    for (key, now) in stream {
        // Timeout check lives here, in the caller, before the new key is judged:
        // a too-slow key means the held prefix was abandoned, so flush it (pass
        // its keys through as literals) and let this key start fresh.
        if let Some(prev) = last {
            if now.saturating_sub(prev) > WINDOW && !pending.is_empty() {
                let dropped = pending.flush();
                println!(
                    "{} @ {now:?} -> idle ({:?} gap > {WINDOW:?}), flushed as literals",
                    render(&dropped),
                    now.saturating_sub(prev),
                );
            }
        }

        // `last` tracks the time of the key that last extended a live prefix, so
        // it is set solely by this resolution: only `Pending` leaves a prefix
        // waiting on the clock.
        last = match pending.feed(&map, key) {
            Step::Fired(action) => {
                println!("{key} @ {now:?} -> fire {action:?}");
                None
            }
            Step::Pending => {
                println!("{key} @ {now:?} -> prefix, waiting (window {WINDOW:?})");
                Some(now)
            }
            Step::PassThrough(keys) => {
                println!("{} @ {now:?} -> no binding, passing through", render(&keys));
                None
            }
        };
    }

    // The stream ended mid-prefix. A real caller's idle timer would fire after the
    // window with no further key; here we flush that dangling `j` as a literal —
    // the step the old version of this example could only describe in a comment.
    let dangling = pending.flush();
    if !dangling.is_empty() {
        println!(
            "{} -> still pending at end; idle timer flushes it as a literal",
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
