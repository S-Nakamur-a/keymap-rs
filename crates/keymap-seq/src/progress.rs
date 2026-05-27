//! Caller-owned progress through a [`SequenceKeymap`].

use keymap_core::KeyInput;

use crate::{Match, SequenceKeymap};

/// A caller-owned pending buffer for resolving multi-key sequences, with the
/// clear/retain bookkeeping folded in.
///
/// [`SequenceKeymap::lookup`] answers "exact / prefix / miss" for the keys so
/// far, but leaves the *caller* to push each key, clear the buffer on a hit or a
/// miss, and keep it on a prefix — the mechanical, easy-to-get-wrong half of
/// `keymap-seq`'s "the caller owns the buffer" contract (see the crate docs and
/// the `leader_sequence` example). `PendingSequence` owns exactly that buffer and
/// that bookkeeping, and nothing else: like the rest of `keymap-rs` it holds **no
/// clock**.
///
/// The idle window a `jj`-style binding needs — "abandon a half-typed prefix if
/// the next key is too slow" — stays the caller's policy. The library cannot see
/// time, so it cannot decide a prefix was abandoned; the caller runs its own
/// timer and calls [`flush`](Self::flush) when that timer fires. Making the idle
/// flush an explicit method (rather than a buffer the caller forgets to drain) is
/// the point: it is the one step the old example could only describe in a comment.
///
/// It is generic over no lifetime — the [`SequenceKeymap`] is passed to
/// [`feed`](Self::feed) per call rather than borrowed for the buffer's lifetime —
/// so a `PendingSequence` can live as a field beside the map it reads. Holding a
/// borrow would make that a self-referential struct, which is exactly the layout
/// a modal-UI caller wants (`struct App { map: SequenceKeymap<A>, pending:
/// PendingSequence }`).
///
/// ```
/// use keymap_core::{Key, KeyInput, Modifiers};
/// use keymap_seq::{PendingSequence, SequenceKeymap, Step};
///
/// #[derive(Debug, PartialEq)]
/// enum Action {
///     Save,
/// }
///
/// let cx = KeyInput::new(Key::Char('x'), Modifiers::CTRL);
/// let cs = KeyInput::new(Key::Char('s'), Modifiers::CTRL);
/// let mut map = SequenceKeymap::new();
/// map.bind([cx, cs], Action::Save).unwrap();
///
/// let mut pending = PendingSequence::new();
/// assert!(matches!(pending.feed(&map, cx), Step::Pending));
/// assert!(matches!(pending.feed(&map, cs), Step::Fired(&Action::Save)));
/// // Fired cleared the buffer, so the next key starts a fresh sequence.
/// assert!(pending.is_empty());
/// ```
#[derive(Debug, Clone, Default)]
pub struct PendingSequence {
    pending: Vec<KeyInput>,
}

impl PendingSequence {
    /// Creates an empty pending buffer.
    #[must_use]
    pub fn new() -> Self {
        PendingSequence::default()
    }

    /// Pushes `key` onto the buffer, resolves the buffer against `map`, and
    /// returns what the caller should do — applying the clear/retain bookkeeping
    /// itself.
    ///
    /// This is the [`SequenceKeymap::lookup`] trichotomy turned into an action:
    /// - [`Match::Exact`] → [`Step::Fired`], and the buffer is **cleared** (the
    ///   sequence completed; the next `feed` starts fresh).
    /// - [`Match::Prefix`] → [`Step::Pending`], and the buffer is **kept** (more
    ///   keys may follow; (re)start your idle timer).
    /// - [`Match::NoMatch`] → [`Step::PassThrough`] carrying the **whole** buffer
    ///   (every key buffered so far, in order), and the buffer is **cleared**.
    ///
    /// The whole buffer — not just `key` — is returned on a miss because the
    /// earlier keys were held on the promise of a longer binding that did not
    /// materialise, so they are now the caller's to handle (replay, pass to the
    /// PTY, …). A key that would have started its own sequence but arrived
    /// mid-prefix is passed through with the rest, not re-fed: "is this key a
    /// fresh start or the tail of an abandoned prefix?" is lookahead policy, which
    /// stays the caller's (the same reason the library has no clock).
    pub fn feed<'a, A>(&mut self, map: &'a SequenceKeymap<A>, key: KeyInput) -> Step<'a, A> {
        self.pending.push(key);
        match map.lookup(&self.pending) {
            Match::Exact(action) => {
                self.pending.clear();
                Step::Fired(action)
            }
            Match::Prefix => Step::Pending,
            Match::NoMatch => Step::PassThrough(std::mem::take(&mut self.pending)),
        }
    }

    /// Drains the pending buffer, returning the keys held in it (empty if none),
    /// and leaves the buffer empty.
    ///
    /// This is the **idle flush**: call it when your timer says a pending prefix
    /// was abandoned (no further key arrived in time). The returned keys are
    /// literals the caller now owns — the dangling `j` of a half-typed `jj` that
    /// must still reach the application as a plain `j`. It takes no time argument
    /// and consults no window: the decision that the prefix is stale is the
    /// caller's; `flush` only performs the drain the caller decided on. Flushing
    /// an empty buffer is a no-op that returns an empty `Vec`.
    pub fn flush(&mut self) -> Vec<KeyInput> {
        std::mem::take(&mut self.pending)
    }

    /// The keys buffered so far (a proper prefix of some binding), in press order.
    #[must_use]
    pub fn pending(&self) -> &[KeyInput] {
        &self.pending
    }

    /// Returns `true` if no keys are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// What a [`PendingSequence::feed`] resolved to — the caller's next action.
///
/// `Step` is the *action-oriented* dual of [`Match`]: where `Match` states a fact
/// about the table (exact / prefix / miss), `Step` states what the caller does
/// (fire it / keep waiting / take these keys back). The trichotomy is the same
/// closed set — `feed` is a thin translation of the [`SequenceKeymap::lookup`]
/// result into buffer operations, and a pure trie lookup has no fourth outcome —
/// so, like `Match`, this enum is **not** `#[non_exhaustive]`: match all three
/// arms and let the compiler check coverage. (`flush`'s drain is a `Vec`, not a
/// `Step`, so it adds no variant.)
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum Step<'a, A> {
    /// The buffer completed a binding, resolving to this action; the buffer has
    /// been cleared.
    Fired(&'a A),
    /// The buffer is a proper prefix of a longer binding; it has been kept. Keep
    /// reading (and, for a timed binding, (re)start your idle timer).
    Pending,
    /// The buffer is not on any binding path; it has been cleared and its keys —
    /// every key buffered so far, in order — are handed back for the caller to
    /// handle.
    PassThrough(Vec<KeyInput>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use keymap_core::{Key, Modifiers};

    #[derive(Debug, Clone, PartialEq)]
    enum Action {
        Save,
        Quit,
        Top,
        Solo,
    }

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    fn plain(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::NONE)
    }

    /// `ctrl+x ctrl+s` -> Save, `ctrl+x ctrl+c` -> Quit, `g g` -> Top, `q` -> Solo.
    fn sample() -> SequenceKeymap<Action> {
        let mut map = SequenceKeymap::new();
        map.bind([ctrl('x'), ctrl('s')], Action::Save).unwrap();
        map.bind([ctrl('x'), ctrl('c')], Action::Quit).unwrap();
        map.bind([plain('g'), plain('g')], Action::Top).unwrap();
        map.bind([plain('q')], Action::Solo).unwrap();
        map
    }

    #[test]
    fn prefix_then_exact_fires_and_clears() {
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, ctrl('x')), Step::Pending);
        assert_eq!(p.pending(), &[ctrl('x')]);
        assert_eq!(p.feed(&map, ctrl('s')), Step::Fired(&Action::Save));
        // Fired cleared the buffer.
        assert!(p.is_empty());
    }

    #[test]
    fn clear_after_exact_lets_the_next_sequence_start_fresh() {
        // The "clear 忘れ" case, asserted through the *next* transition: a third
        // `g` after a completed `g g` must be a fresh Prefix, not a phantom Exact.
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, plain('g')), Step::Pending);
        assert_eq!(p.feed(&map, plain('g')), Step::Fired(&Action::Top));
        assert_eq!(p.feed(&map, plain('g')), Step::Pending);
        assert_eq!(p.pending(), &[plain('g')]);
    }

    #[test]
    fn passthrough_returns_the_whole_buffer_not_just_the_last_key() {
        // An abandoned prefix (`ctrl+x` then an unrelated key) hands back both
        // keys, in order — dropping the held `ctrl+x` would silently eat input.
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, ctrl('x')), Step::Pending);
        assert_eq!(
            p.feed(&map, plain('z')),
            Step::PassThrough(vec![ctrl('x'), plain('z')])
        );
        assert!(p.is_empty());
    }

    #[test]
    fn length_one_sequence_fires_on_the_first_key() {
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, plain('q')), Step::Fired(&Action::Solo));
        assert!(p.is_empty());
    }

    #[test]
    fn empty_map_passes_every_key_through() {
        let map: SequenceKeymap<Action> = SequenceKeymap::new();
        let mut p = PendingSequence::new();
        // No binding can exist, so a key is never a Prefix to wait on.
        assert_eq!(
            p.feed(&map, plain('a')),
            Step::PassThrough(vec![plain('a')])
        );
        assert!(p.is_empty());
    }

    #[test]
    fn abandoned_prefix_does_not_contaminate_a_fresh_sequence() {
        // The example's own stream: an abandoned `ctrl+x z`, then a clean `g g`.
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, ctrl('x')), Step::Pending);
        assert_eq!(
            p.feed(&map, plain('z')),
            Step::PassThrough(vec![ctrl('x'), plain('z')])
        );
        assert_eq!(p.feed(&map, plain('g')), Step::Pending);
        assert_eq!(p.feed(&map, plain('g')), Step::Fired(&Action::Top));
        assert!(p.is_empty());
    }

    #[test]
    fn a_key_that_could_start_a_sequence_is_passed_through_mid_prefix() {
        // `a b` and `c` bound; `a` then `c` makes `[a, c]` a miss — `c` is handed
        // back *with* `a`, not re-fed as a fresh start (lookahead is the caller's).
        let mut map = SequenceKeymap::new();
        map.bind([plain('a'), plain('b')], Action::Save).unwrap();
        map.bind([plain('c')], Action::Solo).unwrap();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, plain('a')), Step::Pending);
        assert_eq!(
            p.feed(&map, plain('c')),
            Step::PassThrough(vec![plain('a'), plain('c')])
        );
    }

    #[test]
    fn branching_prefix_reaches_both_leaves() {
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, ctrl('x')), Step::Pending);
        assert_eq!(p.feed(&map, ctrl('c')), Step::Fired(&Action::Quit));
        assert_eq!(p.feed(&map, ctrl('x')), Step::Pending);
        assert_eq!(p.feed(&map, ctrl('s')), Step::Fired(&Action::Save));
    }

    #[test]
    fn deep_prefix_is_pending_at_every_step_then_fires() {
        let mut map = SequenceKeymap::new();
        map.bind([plain('a'), plain('b'), plain('c')], Action::Top)
            .unwrap();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, plain('a')), Step::Pending);
        assert_eq!(p.feed(&map, plain('b')), Step::Pending);
        assert_eq!(p.feed(&map, plain('c')), Step::Fired(&Action::Top));
    }

    // --- flush: the idle-flush litmus and its abnormal cases. ---

    #[test]
    fn flush_drains_a_pending_prefix_as_literals_exactly_once() {
        // The litmus the old example could only narrate: a prefix held, then the
        // caller's idle timer flushes the dangling key as a literal, once, and the
        // buffer ends empty.
        let mut map = SequenceKeymap::new();
        map.bind([plain('j'), plain('j')], Action::Top).unwrap();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, plain('j')), Step::Pending);
        assert_eq!(p.pending(), &[plain('j')]);
        assert_eq!(p.flush(), vec![plain('j')]);
        assert!(p.is_empty());
        // Idempotent: a second flush has nothing left to drain.
        assert_eq!(p.flush(), Vec::<KeyInput>::new());
    }

    #[test]
    fn flush_on_empty_buffer_is_a_no_op() {
        let mut p = PendingSequence::new();
        assert_eq!(p.flush(), Vec::<KeyInput>::new());
        assert!(p.is_empty());
    }

    #[test]
    fn flush_after_fired_or_passthrough_is_empty() {
        // Fired and PassThrough both already cleared the buffer, so flush must not
        // replay their keys (which would double-emit them).
        let map = sample();
        let mut p = PendingSequence::new();
        assert_eq!(p.feed(&map, plain('q')), Step::Fired(&Action::Solo)); // cleared
        assert_eq!(p.flush(), Vec::<KeyInput>::new());
        assert_eq!(
            p.feed(&map, plain('z')),
            Step::PassThrough(vec![plain('z')])
        ); // cleared
        assert_eq!(p.flush(), Vec::<KeyInput>::new());
    }

    #[test]
    fn no_key_is_dropped_or_duplicated_across_a_stream() {
        // Conservation law (Theresa): the keys returned by PassThrough + flush,
        // plus the keys consumed by each Fired sequence, equal the input stream in
        // order, with nothing lost or repeated. Catches clear-bugs and the
        // mid-prefix pass-through edge in one assertion.
        let map = sample();
        let stream = [
            ctrl('x'),
            ctrl('s'), // completes Save: consumes [ctrl+x, ctrl+s]
            ctrl('x'),
            plain('z'), // miss: passes through [ctrl+x, z]
            plain('g'), // dangling prefix at end: flushed
        ];
        let mut p = PendingSequence::new();
        let mut seen: Vec<KeyInput> = Vec::new();
        let mut buf: Vec<KeyInput> = Vec::new();
        for key in stream {
            buf.push(key);
            match p.feed(&map, key) {
                Step::Fired(_) => {
                    seen.append(&mut buf);
                }
                Step::Pending => {}
                Step::PassThrough(keys) => {
                    // The buffered keys must equal what we tracked independently.
                    assert_eq!(keys, buf);
                    seen.append(&mut buf);
                }
            }
        }
        // Whatever is still pending is what flush will hand back.
        let mut flushed = p.flush();
        seen.append(&mut flushed);
        buf.clear();
        assert_eq!(seen, stream);
    }
}
