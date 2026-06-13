//! Time-windowed sequence resolution.
//!
//! [`TimedPending`] wraps [`PendingSequence`] and bundles the expiry check,
//! buffer flush, and new-key processing into a single [`feed`](TimedPending::feed)
//! call — absorbing the ~30-line event-loop pattern that every `jj`-style caller
//! previously hand-wrote.
//!
//! **The library never calls `Instant::now()`** — the current instant is
//! caller-supplied data, exactly as the active-layer chain and the pending key
//! buffer are caller-owned state. Providing a wrapper type is within the
//! state-free contract because the *decision* (is the prefix stale?) is still
//! the caller's clock datum, not a hidden wall-clock read.

use std::time::{Duration, Instant};

use keymap_core::KeyInput;

use crate::{PendingSequence, SequenceKeymap, Step};

/// A [`PendingSequence`] with a time-window expiry gate.
///
/// Each call to [`feed`](Self::feed) performs three things in order:
/// 1. If the pending buffer is non-empty and the gap since the last accepted key
///    exceeds `window`, the old buffer is drained, and the result's
///    [`expired`](TimedStep::expired) field carries those keys (guaranteed
///    non-empty when `Some`).
/// 2. The buffer (now empty if it just expired) accepts the new `key`.
/// 3. The new-key [`Step`] is returned in [`TimedStep::step`].
///
/// **The library does not call `Instant::now()`**; `now` is caller data
/// (see the crate docs and [`PendingSequence`]).
///
/// ## Layout
///
/// `TimedPending` is designed to live beside the map it reads:
///
/// ```
/// use keymap_seq::{SequenceKeymap, TimedPending};
///
/// struct App<A> {
///     map: SequenceKeymap<A>,
///     pending: TimedPending,
/// }
/// ```
///
/// ## Example
///
/// ```
/// use std::time::{Duration, Instant};
/// use keymap_core::{Key, KeyInput, Modifiers};
/// use keymap_seq::{SequenceKeymap, Step, TimedPending, TimedStep};
///
/// #[derive(Debug, PartialEq)]
/// enum Action { NormalMode }
///
/// let j = KeyInput::new(Key::Char('j'), Modifiers::NONE);
/// let mut map = SequenceKeymap::new();
/// map.bind([j, j], Action::NormalMode).unwrap();
///
/// const WINDOW: Duration = Duration::from_millis(500);
/// let base = Instant::now();
/// let mut pending = TimedPending::new();
///
/// // First j: still a prefix, no expiry yet.
/// let r1 = pending.feed(&map, j, base, WINDOW);
/// assert!(r1.expired.is_none());
/// assert!(matches!(r1.step, Step::Pending));
///
/// // Second j arrives within the window: fires.
/// let r2 = pending.feed(&map, j, base + Duration::from_millis(100), WINDOW);
/// assert!(r2.expired.is_none());
/// assert!(matches!(r2.step, Step::Fired(&Action::NormalMode)));
/// ```
#[derive(Debug, Clone, Default)]
pub struct TimedPending {
    inner: PendingSequence,
    /// The instant of the last key that extended the live prefix.
    /// `None` when the buffer is empty.
    last_key_at: Option<Instant>,
}

impl TimedPending {
    /// Creates an empty timed pending buffer.
    #[must_use]
    pub fn new() -> Self {
        TimedPending::default()
    }

    /// Processes one key with expiry checking, returning the combined result.
    ///
    /// The call sequence is:
    /// 1. **Expiry check**: if the pending buffer is non-empty and
    ///    `now - last_key_at > window`, the buffer is flushed. The flushed keys
    ///    appear in [`TimedStep::expired`] (always non-empty when `Some`).
    /// 2. **Key processing**: `key` is fed into the (possibly just-reset) buffer
    ///    via [`PendingSequence::feed`], and the result becomes
    ///    [`TimedStep::step`].
    /// 3. **Clock update**: `last_key_at` is set to `now` when the step is
    ///    [`Step::Pending`] (the buffer grew), or cleared otherwise.
    ///
    /// The library never calls `Instant::now()`. Pass the event timestamp from
    /// your event loop.
    pub fn feed<'a, A>(
        &mut self,
        map: &'a SequenceKeymap<A>,
        key: KeyInput,
        now: Instant,
        window: Duration,
    ) -> TimedStep<'a, A> {
        // Step 1: expiry check.
        let expired = if let Some(last) = self.last_key_at {
            if now.duration_since(last) > window && !self.inner.is_empty() {
                let keys = self.inner.flush();
                self.last_key_at = None;
                // keys is always non-empty here: we checked `!self.inner.is_empty()`
                // above and `flush` drains exactly what was pending.
                debug_assert!(!keys.is_empty(), "flushed buffer must be non-empty");
                Some(keys)
            } else {
                None
            }
        } else {
            None
        };

        // Step 2: feed the new key.
        let step = self.inner.feed(map, key);

        // Step 3: update the clock.
        match &step {
            Step::Pending => {
                self.last_key_at = Some(now);
            }
            Step::Fired(_) | Step::PassThrough(_) => {
                self.last_key_at = None;
            }
        }

        TimedStep { expired, step }
    }

    /// The deadline at which a pending prefix expires, or `None` if no keys
    /// are buffered.
    ///
    /// Use this as your event loop's `poll` timeout: if no key arrives before
    /// `deadline`, call [`flush`](Self::flush) and treat the result as
    /// pass-through literals.
    ///
    /// ```
    /// use std::time::{Duration, Instant};
    /// use keymap_core::{Key, KeyInput, Modifiers};
    /// use keymap_seq::{SequenceKeymap, TimedPending};
    ///
    /// #[derive(Debug)] enum Action { Jump }
    /// let j = KeyInput::new(Key::Char('j'), Modifiers::NONE);
    /// let mut map = SequenceKeymap::new();
    /// map.bind([j, j], Action::Jump).unwrap();
    ///
    /// const WINDOW: Duration = Duration::from_millis(500);
    /// let mut pending = TimedPending::new();
    ///
    /// // Empty buffer → no deadline.
    /// assert!(pending.deadline(WINDOW).is_none());
    ///
    /// let base = Instant::now();
    /// pending.feed(&map, j, base, WINDOW);
    /// // After accepting a prefix key, deadline = base + window.
    /// assert_eq!(pending.deadline(WINDOW), Some(base + WINDOW));
    /// ```
    #[must_use]
    pub fn deadline(&self, window: Duration) -> Option<Instant> {
        self.last_key_at.map(|t| t + window)
    }

    /// Drains the pending buffer, returning the keys held in it, and leaves
    /// the buffer empty.
    ///
    /// This is the idle flush: call it when `deadline` has passed with no
    /// further key. The returned keys are literals the caller now owns (the
    /// dangling `j` of a half-typed `jj`). Flushing an empty buffer returns
    /// an empty `Vec`.
    pub fn flush(&mut self) -> Vec<KeyInput> {
        self.last_key_at = None;
        self.inner.flush()
    }

    /// The keys buffered so far, in press order.
    #[must_use]
    pub fn pending(&self) -> &[KeyInput] {
        self.inner.pending()
    }

    /// Returns `true` if no keys are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// The result of a single [`TimedPending::feed`] call.
///
/// This is an **exhaustive struct** — adding a field is a breaking change,
/// chosen deliberately so the compiler catches unhandled cases (the same
/// design principle behind `Match` and `Step` being exhaustive enums). The
/// `#[must_use]` attribute ensures the caller cannot silently discard it.
///
/// ## Exhaustiveness regression test
///
/// The following doctest destructures `TimedStep` without `..` so that adding
/// a new field causes a compile error — making any future field addition a
/// self-aware, visible breaking change:
///
/// ```
/// use keymap_seq::TimedStep;
/// use keymap_seq::Step;
///
/// fn handle<A>(ts: TimedStep<'_, A>) {
///     // Destructure every field — no `..` — so a future field addition
///     // is a compile error (exhaustiveness regression test).
///     let TimedStep { expired, step } = ts;
///     let _ = expired;
///     let _ = step;
/// }
/// ```
#[must_use]
#[derive(Debug)]
pub struct TimedStep<'a, A> {
    /// Keys from the buffer that was discarded because the inter-key gap
    /// exceeded the window. **When `Some`, the `Vec` is guaranteed non-empty.**
    /// `None` means the previous key (if any) arrived within the window.
    pub expired: Option<Vec<KeyInput>>,
    /// The outcome of processing the new key after the expiry check.
    pub step: Step<'a, A>,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use keymap_core::{Key, KeyInput, Modifiers};

    use super::*;
    use crate::{SequenceKeymap, Step};

    fn plain(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::NONE)
    }

    /// A deterministic clock: a fixed base Instant plus Duration offsets.
    /// No `sleep`, no `Instant::now()` in tests.
    fn t(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    const WINDOW: Duration = Duration::from_millis(500);

    /// `[j, j]` → `NormalMode`
    fn jj_map() -> SequenceKeymap<&'static str> {
        let j = plain('j');
        let mut map = SequenceKeymap::new();
        map.bind([j, j], "normal").unwrap();
        map
    }

    // ---- ① within-window continuation ---------------------------------

    #[test]
    fn within_window_step_is_pending_no_expiry() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let r = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        assert!(r.expired.is_none(), "no expiry on first key");
        assert!(matches!(r.step, Step::Pending));
        assert_eq!(tp.pending(), &[plain('j')]);
    }

    #[test]
    fn quick_second_key_fires_with_no_expiry() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        let r = tp.feed(&map, plain('j'), t(base, 100), WINDOW); // 100ms < 500ms
        assert!(r.expired.is_none());
        assert!(matches!(r.step, Step::Fired(&"normal")));
        assert!(tp.is_empty());
    }

    // ---- ② expiry → expired is Some(non-empty) + new key starts fresh --

    #[test]
    fn slow_second_key_produces_expired_and_new_step() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        // 600ms gap > 500ms window → expiry
        let r = tp.feed(&map, plain('j'), t(base, 600), WINDOW);

        // expired carries the old buffer (the first 'j')
        let expired = r.expired.expect("must have expired keys");
        assert!(!expired.is_empty(), "Some(v) must be non-empty");
        assert_eq!(expired, vec![plain('j')]);

        // The new 'j' started a fresh prefix.
        assert!(matches!(r.step, Step::Pending));
        assert_eq!(tp.pending(), &[plain('j')]);
    }

    #[test]
    fn expired_vec_is_never_empty_some() {
        // Invariant: if expired is Some, the Vec is non-empty.
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        let r = tp.feed(&map, plain('j'), t(base, 600), WINDOW);
        if let Some(v) = r.expired {
            assert!(!v.is_empty(), "Some(expired) must never be empty");
        }
    }

    // ---- ③ deadline calculation ----------------------------------------

    #[test]
    fn deadline_after_prefix_equals_last_key_plus_window() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        assert_eq!(tp.deadline(WINDOW), Some(t(base, 0) + WINDOW));
    }

    #[test]
    fn deadline_advances_on_each_pending_key() {
        // With a length-3 sequence: each Pending key updates last_key_at, so
        // deadline reflects the *latest* pending key's timestamp.
        let mut map = SequenceKeymap::new();
        map.bind([plain('a'), plain('b'), plain('c')], "abc")
            .unwrap();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let _ = tp.feed(&map, plain('a'), t(base, 0), WINDOW);
        assert_eq!(tp.deadline(WINDOW), Some(t(base, 0) + WINDOW));

        let _ = tp.feed(&map, plain('b'), t(base, 200), WINDOW);
        assert_eq!(tp.deadline(WINDOW), Some(t(base, 200) + WINDOW));
    }

    // ---- ④ empty buffer deadline = None --------------------------------

    #[test]
    fn deadline_is_none_when_buffer_is_empty() {
        let tp = TimedPending::new();
        assert!(tp.deadline(WINDOW).is_none());
    }

    #[test]
    fn deadline_is_none_after_fired() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();
        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        let _ = tp.feed(&map, plain('j'), t(base, 100), WINDOW); // fires
        assert!(tp.deadline(WINDOW).is_none());
    }

    #[test]
    fn deadline_is_none_after_passthrough() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();
        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        let _ = tp.feed(&map, plain('x'), t(base, 100), WINDOW); // PassThrough
        assert!(tp.deadline(WINDOW).is_none());
    }

    // ---- ⑤ Some ⇒ non-empty invariant (stress) ------------------------

    #[test]
    fn some_expired_is_always_non_empty_across_a_stream() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        let stream = [
            (plain('j'), 0u64),
            (plain('j'), 600),  // expiry: first j, then second j is Prefix
            (plain('j'), 1200), // expiry again: third j expired, fourth starts
            (plain('j'), 1300), // quick: fires
        ];
        for (key, ms) in stream {
            let r = tp.feed(&map, key, t(base, ms), WINDOW);
            if let Some(v) = r.expired {
                assert!(
                    !v.is_empty(),
                    "Some(expired) must never be empty at ms={ms}"
                );
            }
        }
    }

    // ---- ⑥ conservation: no key lost or duplicated ---------------------

    #[test]
    fn no_key_lost_or_duplicated_across_mixed_stream() {
        // A stream with one quick pair (fires), one slow pair (expiry + fresh
        // start), and a trailing flush. Every input key must appear exactly once
        // in the outputs.
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        // j@0, j@100 → fires NormalMode (2 keys consumed)
        // j@700, j@1400 → expiry at second j (j@700 in expired), j@1400 is Prefix
        // trailing flush → j@1400 as literal
        let stream = [
            (plain('j'), 0u64),
            (plain('j'), 100),
            (plain('j'), 700),
            (plain('j'), 1400),
        ];

        let mut output: Vec<KeyInput> = Vec::new();
        for (key, ms) in stream {
            let r = tp.feed(&map, key, t(base, ms), WINDOW);
            if let Some(expired) = r.expired {
                output.extend(expired);
            }
            match r.step {
                Step::Fired(_) => {
                    // The two keys that fired are "consumed" — track them.
                    // We already know which stream position this was; for
                    // conservation we just note the fire consumed 2 keys.
                    // Simpler: add j+j to output to represent them.
                    output.push(plain('j'));
                    output.push(plain('j'));
                }
                Step::PassThrough(keys) => output.extend(keys),
                Step::Pending => {}
            }
        }
        // trailing flush
        output.extend(tp.flush());

        // Every input key must appear exactly once in output: total count matches.
        assert_eq!(output.len(), stream.len());
    }

    // ---- flush ----------------------------------------------------------

    #[test]
    fn flush_drains_pending_and_clears_clock() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();
        let _ = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        assert!(!tp.is_empty());
        assert!(tp.deadline(WINDOW).is_some());
        let flushed = tp.flush();
        assert_eq!(flushed, vec![plain('j')]);
        assert!(tp.is_empty());
        assert!(tp.deadline(WINDOW).is_none());
    }

    #[test]
    fn flush_empty_is_no_op() {
        let mut tp = TimedPending::new();
        assert_eq!(tp.flush(), Vec::<KeyInput>::new());
        assert!(tp.is_empty());
    }

    // ---- ⑦ gap == WINDOW boundary (strict >, not >=) -------------------

    /// A gap of exactly `WINDOW` (500ms) must NOT trigger expiry — only a gap
    /// strictly greater than `WINDOW` does. This pins the `> window` boundary
    /// so that a future change to `>= window` would be caught.
    #[test]
    fn gap_exactly_equal_to_window_does_not_expire() {
        let map = jj_map();
        let base = Instant::now();
        let mut tp = TimedPending::new();

        // First key at t=0 enters a pending prefix.
        let r1 = tp.feed(&map, plain('j'), t(base, 0), WINDOW);
        assert!(r1.expired.is_none(), "no expiry on first key");
        assert!(matches!(r1.step, Step::Pending));

        // Second key at t=500ms (exactly WINDOW). The gap is 500ms which is NOT
        // strictly greater than WINDOW (500ms), so expiry must not fire.
        let r2 = tp.feed(&map, plain('j'), t(base, 500), WINDOW);
        assert!(
            r2.expired.is_none(),
            "gap == WINDOW must not expire (boundary is strict >): expired={:?}",
            r2.expired
        );
        // The two j's form the complete sequence → fires.
        assert!(
            matches!(r2.step, Step::Fired(&"normal")),
            "j j with gap==WINDOW should fire: {:?}",
            r2.step
        );
    }
}
