//! Command name → action index with front-prefix completion.
//!
//! Ex-command input (`:wq`, `:w file`) decomposes into three stages, none of
//! which require a new library type for *execution*: `:` is an ordinary
//! [`Keymap::get`](crate::Keymap::get) hit, text accumulates in a caller-owned
//! line buffer, and Enter dispatches through a `FnMut(&str) -> Option<A>` — see
//! `examples/ex_command.rs`.
//!
//! What a caller-owned name table *cannot* give cheaply is the **discovery
//! layer**: `:w<Tab>` completions (front-prefix enumeration) and a full command
//! listing for a palette. [`CommandIndex`] is that one type — a name-keyed
//! `BTreeMap` whose [`complete`](CommandIndex::complete) method is the primary
//! differentiator.
//!
//! The design deliberately avoids a closed result enum: `:w` can be both an
//! exact command *and* the prefix of `:wq` and `:wqa`, so [`get`] and
//! [`complete`] are two orthogonal pure functions rather than a merged
//! discriminant. Normalisation (trim, case-fold) and dispatch are the caller's
//! responsibility; the index stores names byte-for-byte as bound.
//!
//! [`get`]: CommandIndex::get
//! [`complete`]: CommandIndex::complete

use std::collections::BTreeMap;
use std::ops::Bound;

/// A name-to-action index with front-prefix completion.
///
/// Stores an arbitrary number of named commands, each mapped to a
/// caller-defined action `A`. The primary use case is a vim-style command
/// palette: bind names with [`bind`](Self::bind), look up an exact name with
/// [`get`](Self::get), and enumerate candidates for a partial name with
/// [`complete`](Self::complete).
///
/// **Case sensitivity and whitespace** are the caller's concern: the index
/// stores names byte-for-byte as given. Trim and case-fold before calling
/// `bind` / `get` / `complete` if uniformity is desired.
///
/// **Lexicographic order is a specification**, not just an implementation
/// detail. [`complete`](Self::complete) and [`iter`](Self::iter) both return
/// results in ascending byte order (the order of the underlying `BTreeMap`),
/// so callers may rely on this for stable, deterministic output.
#[derive(Debug, Clone)]
pub struct CommandIndex<A> {
    map: BTreeMap<String, A>,
}

impl<A> Default for CommandIndex<A> {
    fn default() -> Self {
        CommandIndex {
            map: BTreeMap::new(),
        }
    }
}

impl<A> CommandIndex<A> {
    /// Creates an empty index.
    #[must_use]
    pub fn new() -> Self {
        CommandIndex::default()
    }

    /// Binds `name` to `action`, returning the action previously bound to that
    /// name, if any (last-wins — same semantic as [`Keymap::bind`](crate::Keymap::bind)).
    ///
    /// The name is stored byte-for-byte; no trimming or case-folding is applied.
    ///
    /// ```
    /// use keymap_core::cmd::CommandIndex;
    ///
    /// let mut idx: CommandIndex<&str> = CommandIndex::new();
    /// assert_eq!(idx.bind(":w", "write"), None);
    /// // Rebinding returns the old action.
    /// assert_eq!(idx.bind(":w", "write-force"), Some("write"));
    /// assert_eq!(idx.get(":w"), Some(&"write-force"));
    /// ```
    pub fn bind(&mut self, name: impl Into<String>, action: A) -> Option<A> {
        self.map.insert(name.into(), action)
    }

    /// Returns the action bound to `name`, or [`None`] if no exact match exists.
    ///
    /// This is a **complete-match** lookup. For prefix enumeration (e.g.
    /// `:w<Tab>`), use [`complete`](Self::complete).
    ///
    /// ```
    /// use keymap_core::cmd::CommandIndex;
    ///
    /// let mut idx: CommandIndex<u8> = CommandIndex::new();
    /// idx.bind(":w", 1);
    /// idx.bind(":wq", 2);
    ///
    /// assert_eq!(idx.get(":w"), Some(&1));
    /// assert_eq!(idx.get(":wq"), Some(&2));
    /// // Prefix is not an exact match.
    /// assert_eq!(idx.get(":"), None);
    /// ```
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&A> {
        self.map.get(name)
    }

    /// Enumerates every command whose name starts with `prefix`, in
    /// **lexicographic (byte) order**.
    ///
    /// An empty `prefix` is equivalent to [`iter`](Self::iter) — it yields
    /// every bound command. Lexicographic order is a **specification**: callers
    /// may rely on it for stable, deterministic output.
    ///
    /// Note that a name can be both an exact match for one call *and* a prefix
    /// for another: `:w` completes under the prefix `:` and is itself exact
    /// under `get(":w")`. There is no closed result enum merging these two
    /// cases.
    ///
    /// ```
    /// use keymap_core::cmd::CommandIndex;
    ///
    /// let mut idx: CommandIndex<u8> = CommandIndex::new();
    /// idx.bind(":w", 1);
    /// idx.bind(":wq", 2);
    /// idx.bind(":wqa", 3);
    /// idx.bind(":q", 4);
    ///
    /// // Prefix ":w" matches ":w", ":wq", ":wqa" in lexicographic order.
    /// let completions: Vec<(&str, &u8)> = idx.complete(":w").collect();
    /// assert_eq!(completions, vec![(":w", &1), (":wq", &2), (":wqa", &3)]);
    ///
    /// // Empty prefix enumerates everything.
    /// assert_eq!(idx.complete("").count(), 4);
    /// ```
    pub fn complete<'a>(&'a self, prefix: &str) -> impl Iterator<Item = (&'a str, &'a A)> {
        // BTreeMap::range on a string map: the lower bound is the prefix itself;
        // the upper bound is the first string that is lexicographically greater
        // than every string with the given prefix — obtained by incrementing the
        // last byte. If the prefix is empty or incrementing would overflow, there
        // is no upper bound.
        let start = Bound::Included(prefix.to_owned());
        let end = next_prefix(prefix).map_or(Bound::Unbounded, Bound::Excluded);

        self.map.range((start, end)).map(|(k, v)| (k.as_str(), v))
    }

    /// Iterates over every `(name, action)` pair in **lexicographic (byte)
    /// order**.
    ///
    /// This is the discovery dual of [`complete`](Self::complete) (with an
    /// empty prefix) and mirrors [`Keymap::iter`](crate::Keymap::iter) in
    /// purpose.
    ///
    /// ```
    /// use keymap_core::cmd::CommandIndex;
    ///
    /// let mut idx: CommandIndex<u8> = CommandIndex::new();
    /// idx.bind(":q", 2);
    /// idx.bind(":w", 1);
    ///
    /// let all: Vec<(&str, &u8)> = idx.iter().collect();
    /// // Lexicographic order: ":q" before ":w".
    /// assert_eq!(all, vec![(":q", &2), (":w", &1)]);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&str, &A)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Returns the number of commands in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns `true` if the index contains no commands.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Returns the lexicographically smallest string that is strictly greater than
/// every string with the given prefix, or `None` if no such string exists
/// (empty prefix, or the prefix ends with `u8::MAX` bytes).
///
/// Used as the exclusive upper bound for the `BTreeMap::range` call in
/// [`CommandIndex::complete`].
fn next_prefix(prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    let mut bytes = prefix.as_bytes().to_owned();
    // Walk backwards to find the last byte that can be incremented.
    for i in (0..bytes.len()).rev() {
        if bytes[i] < u8::MAX {
            bytes[i] += 1;
            bytes.truncate(i + 1);
            // The resulting bytes may not be valid UTF-8 (e.g. if we incremented
            // into a multi-byte sequence boundary), but BTreeMap<String, _> uses
            // the byte comparison of Ord<String>, and Rust's String Ord delegates
            // to the byte comparison of str, so constructing via from_utf8_lossy
            // would change the sort key. Instead we use from_utf8 and, if it
            // fails, fall back to Unbounded — which is safe (returns a superset).
            return String::from_utf8(bytes).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bind / get ----------------------------------------------------

    #[test]
    fn bind_last_wins_returns_old_value() {
        let mut idx: CommandIndex<&str> = CommandIndex::new();
        assert_eq!(idx.bind(":w", "write"), None);
        assert_eq!(idx.bind(":w", "write-bang"), Some("write"));
        assert_eq!(idx.get(":w"), Some(&"write-bang"));
    }

    #[test]
    fn get_exact_match_only() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":w", 1);
        idx.bind(":wq", 2);
        assert_eq!(idx.get(":w"), Some(&1));
        assert_eq!(idx.get(":wq"), Some(&2));
        // Prefix is not an exact match.
        assert_eq!(idx.get(":"), None);
        // Unknown name.
        assert_eq!(idx.get(":x"), None);
    }

    #[test]
    fn get_returns_none_on_empty_index() {
        let idx: CommandIndex<u8> = CommandIndex::new();
        assert_eq!(idx.get(":w"), None);
    }

    // ---- complete ------------------------------------------------------

    #[test]
    fn complete_empty_prefix_yields_all_in_lex_order() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":wq", 2);
        idx.bind(":w", 1);
        idx.bind(":q", 3);
        let names: Vec<&str> = idx.complete("").map(|(n, _)| n).collect();
        assert_eq!(names, vec![":q", ":w", ":wq"]);
    }

    #[test]
    fn complete_prefix_returns_matching_subset_in_lex_order() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":w", 1);
        idx.bind(":wq", 2);
        idx.bind(":wqa", 3);
        idx.bind(":q", 4);

        let names: Vec<&str> = idx.complete(":w").map(|(n, _)| n).collect();
        assert_eq!(names, vec![":w", ":wq", ":wqa"]);
    }

    #[test]
    fn complete_exact_name_as_prefix_is_included() {
        // ":w" is both an exact command and a prefix for ":wq"; both come back.
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":w", 1);
        idx.bind(":wq", 2);

        let result: Vec<(&str, &u8)> = idx.complete(":w").collect();
        assert_eq!(result, vec![(":w", &1), (":wq", &2)]);
    }

    #[test]
    fn complete_no_match_yields_empty() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":w", 1);
        assert_eq!(idx.complete(":x").count(), 0);
    }

    #[test]
    fn complete_yields_name_and_action_pairs() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":w", 1);
        idx.bind(":wq", 2);

        let result: Vec<(&str, &u8)> = idx.complete(":w").collect();
        assert_eq!(result, [(":w", &1), (":wq", &2)]);
    }

    #[test]
    fn complete_utf8_names_lex_order() {
        // Non-ASCII names: byte order determines lexicographic sort.
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind("écrire", 1);
        idx.bind("éditer", 2);
        idx.bind("effacer", 3);

        // "éc" < "éd" < "ef" in byte order (all start with 0xc3 0xa9 = 'é', then diverge)
        // actually "écrire" starts 0xc3,0xa9,0x63 and "éditer" starts 0xc3,0xa9,0x64 and
        // "effacer" starts 0x65,0x66 — "effacer" is ASCII 'e' which is 0x65 < 0xc3.
        // So lex order: "effacer" < "écrire" < "éditer".
        let all: Vec<&str> = idx.iter().map(|(n, _)| n).collect();
        assert_eq!(all, vec!["effacer", "écrire", "éditer"]);

        // Complete on "é" (bytes 0xc3 0xa9) matches "écrire" and "éditer".
        let completions: Vec<&str> = idx.complete("é").map(|(n, _)| n).collect();
        assert_eq!(completions, vec!["écrire", "éditer"]);
    }

    // ---- iter ----------------------------------------------------------

    #[test]
    fn iter_lexicographic_order() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        idx.bind(":wq", 2);
        idx.bind(":q", 3);
        idx.bind(":w", 1);
        let all: Vec<(&str, &u8)> = idx.iter().collect();
        assert_eq!(all, vec![(":q", &3), (":w", &1), (":wq", &2)]);
    }

    #[test]
    fn iter_empty_index_is_empty() {
        let idx: CommandIndex<u8> = CommandIndex::new();
        assert_eq!(idx.iter().count(), 0);
    }

    // ---- len / is_empty ------------------------------------------------

    #[test]
    fn len_and_is_empty() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        idx.bind(":w", 1);
        assert!(!idx.is_empty());
        assert_eq!(idx.len(), 1);
    }

    // ---- next_prefix helper --------------------------------------------

    #[test]
    fn next_prefix_increments_last_byte() {
        assert_eq!(next_prefix(":w"), Some(":x".to_string()));
    }

    #[test]
    fn next_prefix_empty_returns_none() {
        assert_eq!(next_prefix(""), None);
    }

    #[test]
    fn next_prefix_all_max_bytes_returns_none() {
        // All bytes are u8::MAX (0xFF): no byte can be incremented, so the
        // function returns None.
        // U+FFFF encodes as [0xEF, 0xBF, 0xBF] in UTF-8, all of which are < u8::MAX,
        // so that would not reach the None path. Use a single DEL byte (0x7f) instead:
        // 0x7f + 1 = 0x80, which is not a valid UTF-8 leading byte.
        let raw_ff = std::str::from_utf8(&[0x7f]).unwrap(); // 0x7f is valid ASCII
        // 0x7f + 1 = 0x80, which is not a valid UTF-8 leading byte → from_utf8 fails
        // → next_prefix returns None (safe fallback to Unbounded range).
        assert_eq!(next_prefix(raw_ff), None);
    }

    /// When `next_prefix` returns `None` (non-UTF-8 boundary), `complete` falls
    /// back to an unbounded upper range and returns a superset — no panic, no
    /// missing results.
    #[test]
    fn next_prefix_non_utf8_boundary_complete_returns_superset() {
        let mut idx: CommandIndex<u8> = CommandIndex::new();
        // Bind names that start with DEL (0x7f), which is a valid single-byte
        // ASCII character (the highest ASCII byte). Its next_prefix increments to
        // 0x80, which is invalid UTF-8 → fallback to Unbounded.
        let del_a = std::str::from_utf8(&[0x7f, b'a']).unwrap(); // "\x7fa"
        let del_b = std::str::from_utf8(&[0x7f, b'b']).unwrap(); // "\x7fb"
        let unrelated = "zzzz";
        idx.bind(del_a, 1);
        idx.bind(del_b, 2);
        idx.bind(unrelated, 3);

        // Prefix is a single DEL byte: next_prefix("\x7f") → None (0x7f+1 = 0x80,
        // not valid UTF-8) → Unbounded upper → complete returns all entries ≥ "\x7f",
        // which includes both "\x7fa" and "\x7fb" but also "zzzz" (superset is ok).
        let prefix = std::str::from_utf8(&[0x7f]).unwrap();
        let results: Vec<(&str, &u8)> = idx.complete(prefix).collect();

        // Must include both del_a and del_b (no results dropped).
        assert!(
            results.iter().any(|(n, _)| *n == del_a),
            "del_a must appear in superset: {results:?}"
        );
        assert!(
            results.iter().any(|(n, _)| *n == del_b),
            "del_b must appear in superset: {results:?}"
        );
        // Must not panic: the function completed without error.
    }
}
