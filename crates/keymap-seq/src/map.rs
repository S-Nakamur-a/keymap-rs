//! The sequence table: a prefix-free trie of [`KeyInput`] sequences.

use std::collections::HashMap;

use keymap_core::KeyInput;

/// A table mapping *sequences* of normalized [`KeyInput`]s to a caller-defined
/// action `A`.
///
/// This is the sequence-level analogue of `keymap_core::Keymap`: state-free, with
/// the action type `A` brought by the caller. It is **prefix-free** by
/// construction — see [`SequenceKeymap::bind`] and the crate-level docs — so
/// [`lookup`](SequenceKeymap::lookup) never needs a timeout to disambiguate.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SequenceKeymap<A> {
    root: Node<A>,
}

/// One trie node. The prefix-free invariant is "`action.is_some()` implies
/// `children.is_empty()`": a node that resolves to an action is always a leaf,
/// and a node with children never resolves on its own.
#[derive(Debug, Clone)]
struct Node<A> {
    action: Option<A>,
    children: HashMap<KeyInput, Node<A>>,
}

impl<A> Node<A> {
    fn new() -> Self {
        Node {
            action: None,
            children: HashMap::new(),
        }
    }

    fn is_terminal(&self) -> bool {
        self.action.is_some()
    }
}

/// Appends keys to `seq` while descending from `node` to the first terminal it
/// can reach. `node` is assumed non-terminal; the prefix-free invariant
/// guarantees a non-terminal node has at least one child, so a leaf is always
/// reached. When the node fans out, an arbitrary branch is taken — the caller
/// only needs *one* colliding example.
fn extend_to_terminal<A>(node: &Node<A>, seq: &mut Vec<KeyInput>) {
    let mut cur = node;
    while !cur.is_terminal() {
        let Some((key, child)) = cur.children.iter().next() else {
            break;
        };
        seq.push(*key);
        cur = child;
    }
}

/// Depth-first collects every leaf `(path, action)` under `node`, threading a
/// shared `path` stack (pushed on descent, popped on the way back up) so each
/// recorded path is the exact chord sequence from the root to that leaf. The
/// prefix-free invariant means a node with an action has no children, so no
/// internal node is ever recorded.
fn collect_bindings<'a, A>(
    node: &'a Node<A>,
    path: &mut Vec<KeyInput>,
    out: &mut Vec<(Vec<KeyInput>, &'a A)>,
) {
    if let Some(action) = &node.action {
        out.push((path.clone(), action));
    }
    for (key, child) in &node.children {
        path.push(*key);
        collect_bindings(child, path, out);
        path.pop();
    }
}

impl<A> Default for SequenceKeymap<A> {
    fn default() -> Self {
        SequenceKeymap { root: Node::new() }
    }
}

impl<A> SequenceKeymap<A> {
    /// Creates an empty sequence map.
    #[must_use]
    pub fn new() -> Self {
        SequenceKeymap::default()
    }

    /// Returns `true` if no sequences are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.children.is_empty()
    }

    /// Binds a key `sequence` to `action`.
    ///
    /// Re-binding the *exact* same sequence overwrites and returns the previous
    /// action, mirroring `keymap_core::Keymap::bind`.
    ///
    /// # Errors
    ///
    /// - [`SeqBindError::Empty`] if `sequence` yields no keys.
    /// - [`SeqBindError::PrefixShadow`] if the new sequence would be a proper
    ///   prefix of an existing binding, or an existing binding would be a proper
    ///   prefix of it. Either case is unresolvable without a timeout (see the
    ///   crate docs), so it is rejected rather than silently shadowed; the map is
    ///   left unchanged.
    pub fn bind(
        &mut self,
        sequence: impl IntoIterator<Item = KeyInput>,
        action: A,
    ) -> Result<Option<A>, SeqBindError> {
        let keys: Vec<KeyInput> = sequence.into_iter().collect();
        if keys.is_empty() {
            return Err(SeqBindError::Empty);
        }

        // Descend, creating nodes as needed. Passing *through* an existing
        // terminal means a shorter binding (`keys[..depth]`) is already a prefix
        // of `keys`.
        let mut node = &mut self.root;
        for (depth, key) in keys.iter().enumerate() {
            if node.is_terminal() {
                return Err(SeqBindError::PrefixShadow {
                    sequence: keys.clone(),
                    conflict: keys[..depth].to_vec(),
                });
            }
            node = node.children.entry(*key).or_insert_with(Node::new);
        }

        // `node` is the terminal for `keys`. If it already has children, `keys`
        // is a proper prefix of one or more existing (longer) bindings; surface
        // one of them by descending to any terminal beneath this node.
        if !node.children.is_empty() {
            let mut conflict = keys.clone();
            extend_to_terminal(node, &mut conflict);
            return Err(SeqBindError::PrefixShadow {
                sequence: keys,
                conflict,
            });
        }

        Ok(node.action.replace(action))
    }

    /// Resolves the keys pressed so far against the table.
    ///
    /// `keys` is the caller's pending buffer. The result is one of:
    /// - [`Match::Exact`] — `keys` is a complete binding; the caller should fire
    ///   it and clear the buffer.
    /// - [`Match::Prefix`] — `keys` is a proper prefix of one or more bindings;
    ///   the caller should keep buffering (and, for timed bindings, run its
    ///   timer).
    /// - [`Match::NoMatch`] — `keys` is not on any binding path; the caller
    ///   should clear the buffer and handle the keys itself (e.g. pass through).
    pub fn lookup(&self, keys: &[KeyInput]) -> Match<'_, A> {
        let mut node = &self.root;
        for key in keys {
            match node.children.get(key) {
                Some(child) => node = child,
                None => return Match::NoMatch,
            }
        }
        match &node.action {
            Some(action) => Match::Exact(action),
            None if node.children.is_empty() => Match::NoMatch,
            None => Match::Prefix,
        }
    }

    /// Enumerates every bound sequence as a `(path, action)` pair — the
    /// sequence-level dual of `keymap_core::Keymap::iter`.
    ///
    /// Each yielded `Vec<KeyInput>` is a complete bound sequence (a leaf path);
    /// by the prefix-free invariant it is never a partial prefix. Unlike
    /// `Keymap::iter`, which can borrow its stored key, a trie path is
    /// reconstructed during traversal, so each item owns its `Vec` and borrows
    /// only the action.
    ///
    /// Order is unspecified (the trie's children live in a `HashMap`); collect
    /// and sort caller-side for stable output. This is the data source for
    /// listing, search, and serialization on top of `keymap-seq`.
    pub fn bindings(&self) -> impl Iterator<Item = (Vec<KeyInput>, &A)> {
        let mut out = Vec::new();
        let mut path = Vec::new();
        collect_bindings(&self.root, &mut path, &mut out);
        out.into_iter()
    }

    /// Enumerates the keys that may immediately follow `prefix`, for rendering a
    /// which-key-style menu of what can be pressed next.
    ///
    /// Each item is a [`KeyInput`] that extends `prefix` by one, paired with what
    /// pressing it does: complete a binding ([`Continuation::Action`], carrying
    /// the action) or open a deeper prefix ([`Continuation::Prefix`]).
    ///
    /// The iterator is empty when `prefix` does not name an internal node —
    /// whether it is off the tree, already completes a binding (a leaf has no
    /// children), or the map is empty. `continuations` deliberately does not
    /// re-encode that distinction; call [`lookup`](Self::lookup) to tell those
    /// apart.
    ///
    /// Order is unspecified (the children live in a `HashMap`); collect and sort
    /// caller-side for a stable menu.
    ///
    /// ```
    /// use keymap_core::{Key, KeyInput, Modifiers};
    /// use keymap_seq::{Continuation, SequenceKeymap};
    ///
    /// #[derive(Debug, PartialEq)]
    /// enum Action { Save, Quit }
    ///
    /// let k = |c| KeyInput::new(Key::Char(c), Modifiers::NONE);
    /// let mut map = SequenceKeymap::new();
    /// map.bind([k('a'), k('b')], Action::Save).unwrap();
    /// map.bind([k('c')], Action::Quit).unwrap();
    ///
    /// // From the root: `a` opens a deeper prefix, `c` completes a binding.
    /// let mut next: Vec<_> = map.continuations(&[]).collect();
    /// next.sort_by_key(|(key, _)| key.to_string());
    /// assert_eq!(
    ///     next,
    ///     vec![
    ///         (k('a'), Continuation::Prefix),
    ///         (k('c'), Continuation::Action(&Action::Quit)),
    ///     ],
    /// );
    ///
    /// // A completed binding has no continuations.
    /// assert_eq!(map.continuations(&[k('c')]).count(), 0);
    /// ```
    pub fn continuations(
        &self,
        prefix: &[KeyInput],
    ) -> impl Iterator<Item = (KeyInput, Continuation<'_, A>)> {
        let mut node = &self.root;
        for key in prefix {
            match node.children.get(key) {
                Some(child) => node = child,
                None => return None.into_iter().flatten(),
            }
        }
        // A child is, by the prefix-free invariant, terminal (an action, no
        // children) xor an internal prefix — so the classification is total.
        Some(node.children.iter().map(|(key, child)| {
            let continuation = child
                .action
                .as_ref()
                .map_or(Continuation::Prefix, Continuation::Action);
            (*key, continuation)
        }))
        .into_iter()
        .flatten()
    }
}

/// The outcome of resolving a pending key buffer with
/// [`SequenceKeymap::lookup`].
///
/// `Resolution` is deliberately *not* the name here: that word is reserved by
/// `keymap-core` for raw-byte passthrough. This is purely the table-view answer
/// — whether the keys are an exact hit, a prefix, or a miss — leaving "should I
/// wait?" to the caller.
///
/// The three variants are exhaustive on purpose: a pure trie lookup has no other
/// outcome, and timeout (`jj`-style) handling lives in the caller, not in this
/// result. So this enum is *not* `#[non_exhaustive]` — callers can and should
/// match all three arms with the compiler checking coverage.
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum Match<'a, A> {
    /// The keys are a complete binding, resolving to this action.
    Exact(&'a A),
    /// The keys are a proper prefix of at least one longer binding.
    Prefix,
    /// The keys are not on any binding path.
    NoMatch,
}

/// What pressing one more key after a prefix does, as reported by
/// [`SequenceKeymap::continuations`].
///
/// The two variants are exhaustive on purpose, mirroring [`Match`]: by the
/// prefix-free invariant a node is terminal *xor* internal, so a child is either
/// a completed binding or a deeper prefix — there is no third kind of node. This
/// enum is therefore *not* `#[non_exhaustive]`; a third variant could only arise
/// by abandoning the trie's defining invariant.
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum Continuation<'a, A> {
    /// Pressing this key completes a binding, resolving to this action.
    Action(&'a A),
    /// Pressing this key opens a deeper prefix; more keys must follow.
    Prefix,
}

/// Why a [`SequenceKeymap::bind`] call was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SeqBindError {
    /// The sequence contained no keys.
    Empty,
    /// The sequence collides with an existing binding on a shared prefix: one is
    /// a proper prefix of the other, which cannot be resolved without a timeout.
    PrefixShadow {
        /// The sequence whose binding was rejected.
        sequence: Vec<KeyInput>,
        /// An existing binding it collides with — one of `sequence`/`conflict`
        /// is a proper prefix of the other. When several existing bindings share
        /// the prefix, this names one of them.
        conflict: Vec<KeyInput>,
    },
}

impl core::fmt::Display for SeqBindError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SeqBindError::Empty => f.write_str("empty key sequence"),
            SeqBindError::PrefixShadow { .. } => {
                f.write_str("key sequence shares a prefix with an existing binding")
            }
        }
    }
}

impl std::error::Error for SeqBindError {}

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

    /// `ctrl+x ctrl+s` -> Save, `ctrl+x ctrl+c` -> Quit, `g g` -> Top,
    /// `q` (length-1 sequence) -> Solo.
    fn sample() -> SequenceKeymap<Action> {
        let mut map = SequenceKeymap::new();
        map.bind([ctrl('x'), ctrl('s')], Action::Save).unwrap();
        map.bind([ctrl('x'), ctrl('c')], Action::Quit).unwrap();
        map.bind([plain('g'), plain('g')], Action::Top).unwrap();
        map.bind([plain('q')], Action::Solo).unwrap();
        map
    }

    #[test]
    fn single_key_sequence_resolves() {
        let map = sample();
        assert_eq!(map.lookup(&[plain('q')]), Match::Exact(&Action::Solo));
    }

    #[test]
    fn prefix_then_leaf() {
        let map = sample();
        // One key in: still a prefix.
        assert_eq!(map.lookup(&[ctrl('x')]), Match::Prefix);
        // Full sequence: exact.
        assert_eq!(
            map.lookup(&[ctrl('x'), ctrl('s')]),
            Match::Exact(&Action::Save)
        );
    }

    #[test]
    fn branching_prefix_reaches_both_leaves() {
        let map = sample();
        assert_eq!(
            map.lookup(&[ctrl('x'), ctrl('c')]),
            Match::Exact(&Action::Quit)
        );
        assert_eq!(
            map.lookup(&[ctrl('x'), ctrl('s')]),
            Match::Exact(&Action::Save)
        );
    }

    #[test]
    fn miss_on_first_key() {
        let map = sample();
        assert_eq!(map.lookup(&[plain('z')]), Match::NoMatch);
    }

    #[test]
    fn miss_off_a_prefix_path() {
        let map = sample();
        // `ctrl+x` is a valid prefix, but `ctrl+x z` leaves the tree.
        assert_eq!(map.lookup(&[ctrl('x'), plain('z')]), Match::NoMatch);
    }

    #[test]
    fn empty_buffer_on_nonempty_map_is_prefix() {
        let map = sample();
        // Nothing typed yet, but bindings exist: still possible.
        assert_eq!(map.lookup(&[]), Match::Prefix);
    }

    #[test]
    fn empty_buffer_on_empty_map_is_no_match() {
        let map: SequenceKeymap<Action> = SequenceKeymap::new();
        assert!(map.is_empty());
        assert_eq!(map.lookup(&[]), Match::NoMatch);
        assert_eq!(map.lookup(&[ctrl('x')]), Match::NoMatch);
    }

    #[test]
    fn rebinding_exact_sequence_overwrites_and_returns_previous() {
        let mut map = SequenceKeymap::new();
        assert_eq!(map.bind([ctrl('x'), ctrl('s')], Action::Save), Ok(None));
        assert_eq!(
            map.bind([ctrl('x'), ctrl('s')], Action::Quit),
            Ok(Some(Action::Save))
        );
        assert_eq!(
            map.lookup(&[ctrl('x'), ctrl('s')]),
            Match::Exact(&Action::Quit)
        );
    }

    #[test]
    fn empty_sequence_is_rejected() {
        let mut map: SequenceKeymap<Action> = SequenceKeymap::new();
        assert_eq!(map.bind([], Action::Save), Err(SeqBindError::Empty));
    }

    #[test]
    fn existing_short_binding_shadows_new_longer_one() {
        let mut map = SequenceKeymap::new();
        map.bind([plain('g')], Action::Solo).unwrap();
        // `g` is already terminal; `g g` would pass through it.
        assert_eq!(
            map.bind([plain('g'), plain('g')], Action::Top),
            Err(SeqBindError::PrefixShadow {
                sequence: vec![plain('g'), plain('g')],
                conflict: vec![plain('g')],
            })
        );
        // The map is unchanged: `g` still resolves, `g g` was never added.
        assert_eq!(map.lookup(&[plain('g')]), Match::Exact(&Action::Solo));
    }

    #[test]
    fn new_short_binding_shadows_existing_longer_one() {
        let mut map = SequenceKeymap::new();
        map.bind([plain('g'), plain('g')], Action::Top).unwrap();
        // `g` would be a prefix of the existing `g g`.
        assert_eq!(
            map.bind([plain('g')], Action::Solo),
            Err(SeqBindError::PrefixShadow {
                sequence: vec![plain('g')],
                conflict: vec![plain('g'), plain('g')],
            })
        );
        // Unchanged: `g g` still resolves, `g` is only a prefix.
        assert_eq!(map.lookup(&[plain('g')]), Match::Prefix);
        assert_eq!(
            map.lookup(&[plain('g'), plain('g')]),
            Match::Exact(&Action::Top)
        );
    }

    #[test]
    fn deep_sequence_is_prefix_at_every_step_then_exact() {
        // A length-3 sequence: every proper prefix resolves to Prefix, the full
        // sequence to Exact, and a wrong final key to NoMatch.
        let mut map = SequenceKeymap::new();
        map.bind([plain('a'), plain('b'), plain('c')], Action::Top)
            .unwrap();
        assert_eq!(map.lookup(&[plain('a')]), Match::Prefix);
        assert_eq!(map.lookup(&[plain('a'), plain('b')]), Match::Prefix);
        assert_eq!(
            map.lookup(&[plain('a'), plain('b'), plain('c')]),
            Match::Exact(&Action::Top)
        );
        assert_eq!(
            map.lookup(&[plain('a'), plain('b'), plain('z')]),
            Match::NoMatch
        );
    }

    #[test]
    fn independent_single_and_sequence_coexist() {
        // `q` (single) and `ctrl+x ctrl+s` (sequence) share no prefix, so both
        // bind cleanly — only a *shared* prefix is forbidden.
        let map = sample();
        assert_eq!(map.lookup(&[plain('q')]), Match::Exact(&Action::Solo));
        assert_eq!(map.lookup(&[ctrl('x')]), Match::Prefix);
    }

    /// Collects `continuations` into a vec sorted by the canonical key string, so
    /// the `HashMap`'s unspecified order does not make assertions flaky.
    fn sorted_continuations<'a>(
        map: &'a SequenceKeymap<Action>,
        prefix: &[KeyInput],
    ) -> Vec<(KeyInput, Continuation<'a, Action>)> {
        let mut out: Vec<_> = map.continuations(prefix).collect();
        out.sort_by_key(|(key, _)| key.to_string());
        out
    }

    #[test]
    fn continuations_at_root_list_the_leader_keys() {
        // sample(): `ctrl+x …` (prefix), `g g` (prefix via `g`), `q` (action).
        assert_eq!(
            sorted_continuations(&sample(), &[]),
            vec![
                (ctrl('x'), Continuation::Prefix),
                (plain('g'), Continuation::Prefix),
                (plain('q'), Continuation::Action(&Action::Solo)),
            ]
        );
    }

    #[test]
    fn continuations_after_a_prefix_classify_each_child() {
        // After `ctrl+x`, both `ctrl+s` (Save) and `ctrl+c` (Quit) complete.
        assert_eq!(
            sorted_continuations(&sample(), &[ctrl('x')]),
            vec![
                (ctrl('c'), Continuation::Action(&Action::Quit)),
                (ctrl('s'), Continuation::Action(&Action::Save)),
            ]
        );
    }

    #[test]
    fn continuations_mix_action_and_prefix() {
        // `a b` (action) and `a c d` (deeper) share the prefix `a`: after `a`,
        // `b` completes while `c` opens a further prefix.
        let mut map = SequenceKeymap::new();
        map.bind([plain('a'), plain('b')], Action::Save).unwrap();
        map.bind([plain('a'), plain('c'), plain('d')], Action::Quit)
            .unwrap();
        assert_eq!(
            sorted_continuations(&map, &[plain('a')]),
            vec![
                (plain('b'), Continuation::Action(&Action::Save)),
                (plain('c'), Continuation::Prefix),
            ]
        );
    }

    #[test]
    fn a_completed_binding_has_no_continuations() {
        // `q` is a leaf; nothing follows it.
        assert_eq!(sample().continuations(&[plain('q')]).count(), 0);
        assert_eq!(sample().continuations(&[ctrl('x'), ctrl('s')]).count(), 0);
    }

    #[test]
    fn off_tree_prefix_has_no_continuations() {
        let map = sample();
        // A first key bound nowhere, and a valid prefix then a wrong key.
        assert_eq!(map.continuations(&[plain('z')]).count(), 0);
        assert_eq!(map.continuations(&[ctrl('x'), plain('z')]).count(), 0);
    }

    #[test]
    fn empty_map_has_no_continuations() {
        let map: SequenceKeymap<Action> = SequenceKeymap::new();
        assert_eq!(map.continuations(&[]).count(), 0);
        assert_eq!(map.continuations(&[plain('a')]).count(), 0);
    }

    #[test]
    fn continuations_descend_to_deeper_internal_nodes() {
        // `a b` (action) and `a c d e` (deep) share the prefix `a`; this drives
        // the descent past depth 1 on a *hit* (the other tests stop at depth 1).
        let mut map = SequenceKeymap::new();
        map.bind([plain('a'), plain('b')], Action::Save).unwrap();
        map.bind(
            [plain('a'), plain('c'), plain('d'), plain('e')],
            Action::Quit,
        )
        .unwrap();
        // Depth 2: through `a c` to an internal node whose child `d` is a deeper
        // prefix (a `Prefix` continuation returned away from the root).
        assert_eq!(
            sorted_continuations(&map, &[plain('a'), plain('c')]),
            vec![(plain('d'), Continuation::Prefix)]
        );
        // Depth 3: one key short of the leaf, `e` completes the binding.
        assert_eq!(
            sorted_continuations(&map, &[plain('a'), plain('c'), plain('d')]),
            vec![(plain('e'), Continuation::Action(&Action::Quit))]
        );
    }

    /// Collects `bindings` into a vec sorted by the joined canonical path, so the
    /// `HashMap`'s unspecified order does not make assertions flaky.
    fn sorted_bindings(map: &SequenceKeymap<Action>) -> Vec<(Vec<KeyInput>, &Action)> {
        let mut out: Vec<_> = map.bindings().collect();
        out.sort_by_key(|(path, _)| {
            path.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        });
        out
    }

    #[test]
    fn bindings_enumerate_every_leaf_path() {
        // sample(): two branches under `ctrl+x`, the `g g` leaf, and the `q` leaf.
        assert_eq!(
            sorted_bindings(&sample()),
            vec![
                (vec![ctrl('x'), ctrl('c')], &Action::Quit),
                (vec![ctrl('x'), ctrl('s')], &Action::Save),
                (vec![plain('g'), plain('g')], &Action::Top),
                (vec![plain('q')], &Action::Solo),
            ]
        );
    }

    #[test]
    fn bindings_of_empty_map_is_empty() {
        let map: SequenceKeymap<Action> = SequenceKeymap::new();
        assert_eq!(map.bindings().count(), 0);
    }

    #[test]
    fn bindings_never_yield_internal_prefixes() {
        // Every yielded path must itself resolve to Exact (a leaf) — never a
        // partial prefix. This is the prefix-free invariant, viewed through
        // enumeration.
        let map = sample();
        for (path, action) in map.bindings() {
            assert_eq!(map.lookup(&path), Match::Exact(action));
        }
    }

    #[test]
    fn continuations_nonempty_iff_lookup_is_prefix() {
        // Cross-method invariant: a buffer has continuations exactly when it is a
        // proper prefix; an exact hit or a miss has none. Catches a desync between
        // the two descents without re-deriving the contents (which would be
        // tautological).
        let map = sample();
        let cases: &[&[KeyInput]] = &[
            &[],
            &[plain('q')],
            &[ctrl('x')],
            &[ctrl('x'), ctrl('s')],
            &[plain('z')],
            &[ctrl('x'), plain('z')],
        ];
        for keys in cases {
            let has_next = map.continuations(keys).count() > 0;
            match map.lookup(keys) {
                Match::Prefix => assert!(has_next, "Prefix must have continuations: {keys:?}"),
                Match::Exact(_) | Match::NoMatch => {
                    assert!(!has_next, "non-Prefix must have no continuations: {keys:?}");
                }
            }
        }
    }
}
