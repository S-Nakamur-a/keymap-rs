//! The binding table.

use std::collections::HashMap;

use crate::input::KeyInput;

/// A table mapping normalized [`KeyInput`]s to a caller-defined action `A`.
///
/// `Keymap` is state-free: it answers "what is bound to this input?" and nothing
/// more. Deciding whether to *consume* the action or pass the input through, and
/// tracking modes or multi-key sequences, is the caller's concern (and the job
/// of later layers). A lookup miss is an absence ([`None`]), not an error — the
/// caller treats it as "pass through".
///
/// The action type `A` is yours. The struct itself places no bound on `A`;
/// bounds appear only on the methods that need them.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Keymap<A> {
    bindings: HashMap<KeyInput, A>,
}

impl<A> Default for Keymap<A> {
    fn default() -> Self {
        Keymap {
            bindings: HashMap::new(),
        }
    }
}

impl<A> Keymap<A> {
    /// Creates an empty keymap.
    #[must_use]
    pub fn new() -> Self {
        Keymap::default()
    }

    /// Binds `input` to `action`, returning the action previously bound to that
    /// input, if any.
    pub fn bind(&mut self, input: KeyInput, action: A) -> Option<A> {
        self.bindings.insert(input, action)
    }

    /// Removes any binding for `input`, returning the action that was bound.
    pub fn unbind(&mut self, input: &KeyInput) -> Option<A> {
        self.bindings.remove(input)
    }

    /// Returns the action bound to `input`, or [`None`] if the input is unbound.
    #[must_use]
    pub fn get(&self, input: &KeyInput) -> Option<&A> {
        self.bindings.get(input)
    }

    /// Returns `true` if `input` has a binding.
    #[must_use]
    pub fn contains(&self, input: &KeyInput) -> bool {
        self.bindings.contains_key(input)
    }

    /// The number of bindings in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Returns `true` if the table has no bindings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Iterates over every `(input, action)` binding.
    ///
    /// Order is unspecified. This is the data source for listing and search
    /// (the discovery layer) built on top of `keymap-core`.
    pub fn iter(&self) -> impl Iterator<Item = (&KeyInput, &A)> {
        self.bindings.iter()
    }
}

/// Resolves `input` against an ordered list of keymap layers, returning the
/// first match (earlier layers win).
///
/// This is how context-dependent bindings are expressed without the library
/// knowing anything about "contexts": the *caller* decides, from its own state,
/// which layers are active and in what priority order, then passes them here.
/// A binding present only in a high-priority layer overrides the base; a binding
/// only in the base falls through and still resolves. The library never sees a
/// mode or context type — only an ordered sequence of plain [`Keymap`]s.
///
/// This is a lexical scope chain (`block → panel → global`, innermost-first):
/// the caller flattens its context *tree* into this ordered list per event, and
/// the list stays fixed during resolution, so the resolver is state-free. A
/// miss in every layer ([`None`]) is the "pass through" signal — in a terminal
/// multiplexer that is the key flowing to the PTY, which lives *past the end of
/// the chain* as a sink, never as a layer within it.
///
/// ```
/// use keymap_core::{resolve_layered, Key, KeyInput, Keymap, Modifiers};
///
/// #[derive(PartialEq, Debug)]
/// enum Action { Save, Split, Quit }
///
/// let ctrl = |c| KeyInput::new(Key::Char(c), Modifiers::CTRL);
/// let mut base = Keymap::new();
/// base.bind(ctrl('s'), Action::Save);
/// base.bind(ctrl('q'), Action::Quit);
/// let mut panel = Keymap::new();
/// panel.bind(ctrl('s'), Action::Split);
///
/// // In "panel" context, the panel layer overrides ctrl+s …
/// assert_eq!(resolve_layered([&panel, &base], &ctrl('s')), Some(&Action::Split));
/// // … but ctrl+q falls through to the base layer.
/// assert_eq!(resolve_layered([&panel, &base], &ctrl('q')), Some(&Action::Quit));
/// // In "editor" context, only the base layer is active.
/// assert_eq!(resolve_layered([&base], &ctrl('s')), Some(&Action::Save));
/// ```
pub fn resolve_layered<'a, A, L>(layers: L, input: &KeyInput) -> Option<&'a A>
where
    L: IntoIterator<Item = &'a Keymap<A>>,
    A: 'a,
{
    layers.into_iter().find_map(|layer| layer.get(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Key, Modifiers};

    #[derive(Debug, Clone, PartialEq)]
    enum Action {
        Quit,
        Save,
        Split,
    }

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    #[test]
    fn bind_then_get() {
        let mut map = Keymap::new();
        assert!(map.is_empty());
        assert_eq!(map.bind(ctrl('q'), Action::Quit), None);
        assert_eq!(map.get(&ctrl('q')), Some(&Action::Quit));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn miss_is_none_not_error() {
        let map: Keymap<Action> = Keymap::new();
        assert_eq!(map.get(&ctrl('q')), None);
        assert!(!map.contains(&ctrl('q')));
    }

    #[test]
    fn rebinding_returns_previous_action() {
        let mut map = Keymap::new();
        map.bind(ctrl('s'), Action::Save);
        let previous = map.bind(ctrl('s'), Action::Quit);
        assert_eq!(previous, Some(Action::Save));
        assert_eq!(map.get(&ctrl('s')), Some(&Action::Quit));
    }

    #[test]
    fn unbind_removes() {
        let mut map = Keymap::new();
        map.bind(ctrl('q'), Action::Quit);
        assert_eq!(map.unbind(&ctrl('q')), Some(Action::Quit));
        assert_eq!(map.get(&ctrl('q')), None);
    }

    #[test]
    fn modifiers_are_part_of_the_key() {
        let mut map = Keymap::new();
        map.bind(ctrl('q'), Action::Quit);
        // Same character, no modifier -> different binding slot.
        let plain = KeyInput::new(Key::Char('q'), Modifiers::NONE);
        assert_eq!(map.get(&plain), None);
    }

    #[test]
    fn layered_resolution_prefers_earlier_layers_and_falls_through() {
        let mut base = Keymap::new();
        base.bind(ctrl('s'), Action::Save);
        base.bind(ctrl('q'), Action::Quit);
        let mut overlay = Keymap::new();
        overlay.bind(ctrl('s'), Action::Split);

        // Overlay wins for ctrl+s …
        assert_eq!(
            resolve_layered([&overlay, &base], &ctrl('s')),
            Some(&Action::Split)
        );
        // … ctrl+q falls through to base …
        assert_eq!(
            resolve_layered([&overlay, &base], &ctrl('q')),
            Some(&Action::Quit)
        );
        // … and without the overlay, base resolves ctrl+s.
        assert_eq!(resolve_layered([&base], &ctrl('s')), Some(&Action::Save));
    }

    #[test]
    fn layered_resolution_misses_when_no_layer_binds() {
        let base: Keymap<Action> = Keymap::new();
        assert_eq!(resolve_layered([&base], &ctrl('x')), None);
        // No layers at all is a miss, not a panic.
        assert_eq!(
            resolve_layered(std::iter::empty::<&Keymap<Action>>(), &ctrl('x')),
            None
        );
    }
}
