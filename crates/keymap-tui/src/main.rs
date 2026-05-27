//! `keymap-tui` — a live inspector for keymap-rs decoding and layered resolution.
//!
//! Run it in each terminal you want to check (iTerm2, Alacritty, kitty, …),
//! optionally under tmux or over SSH:
//!
//! ```sh
//! cargo run -p keymap-tui
//! ```
//!
//! For every key you press it shows, side by side:
//! 1. what **crossterm** reports (the raw event: code + modifiers),
//! 2. what **keymap-core** decodes it to (`KeyInput` via `TryFrom`, or
//!    "unsupported"),
//! 3. how the key is **dispatched** through the scope chain in the current
//!    context — which layer resolved it, or that it fell through to the sink.
//!
//! ## The scope chain
//!
//! Context-dependent bindings are a lexical scope chain: `block → panel →
//! global`, resolved innermost-first, first hit wins, a miss falls outward. The
//! library is `resolve_layered` (a pure first-hit fold over an ordered layer
//! list); the *which layer won* provenance shown here is computed caller-side by
//! [`resolve_with_layer`] so the core resolver stays `Option<&A>`.
//!
//! Two special positions sit outside the chain:
//! - **reserved** (e.g. `ctrl+c`) wins over *every* layer — it is checked before
//!   resolution, so it can never be swallowed by a binding (the escape hatch).
//! - the **PTY sink** receives anything no layer binds (`None`). It is *past the
//!   end* of the chain, not a layer in it, so it cannot shadow the layers above.
//!
//! ## Toggles
//! - **F2** moves your context inward/outward (`global → panel → block`),
//!   activating more layers. Watch `ctrl+s` resolve `via global` → `via panel`
//!   → `via block` as you descend; `ctrl+w` (panel-only) and `ctrl+q`
//!   (global-only) show fall-through.
//! - **F3** toggles the kitty keyboard protocol (if supported). Watch `ctrl+i`
//!   (equals `Tab` in legacy encodings) and `cmd+1` change as you toggle it.

use std::fmt::Write as _;
use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use keymap_config::BuildOutput;
use keymap_core::{KeyInput, Keymap};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::Line,
    widgets::{Block, List, ListItem, Paragraph},
};

// `global` is the rich base (it also carries the decode-inspection keys: tab,
// ctrl+i, arrows, super+1). `panel` and `block` are thin overrides that
// demonstrate the scope chain. `ctrl+s` is bound in all three with *distinct*
// actions, so provenance and action must agree; `ctrl+c` is bound here yet is
// pre-empted by the reserved escape hatch.
const GLOBAL_TOML: &str = r#"
[keys]
"ctrl+s" = "save"
"ctrl+shift+s" = "save_as"
"ctrl+q" = "quit"
"ctrl+c" = "cancel"
"f1" = "help"
"ctrl+i" = "focus_next"
"tab" = "indent"
"shift+tab" = "outdent"
"up" = "move_up"
"down" = "move_down"
"super+1" = "first_tab"
"#;

// panel-only: `ctrl+w`. Overrides `ctrl+s`.
const PANEL_TOML: &str = r#"
[keys]
"ctrl+s" = "split_panel"
"ctrl+w" = "close_panel"
"#;

// block-only: overrides `ctrl+s` at the innermost layer.
const BLOCK_TOML: &str = r#"
[keys]
"ctrl+s" = "split_block"
"#;

const MAX_LOG_ROWS: usize = 14;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Save,
    SaveAs,
    Quit,
    Cancel,
    Help,
    FocusNext,
    Indent,
    Outdent,
    MoveUp,
    MoveDown,
    FirstTab,
    SplitPanel,
    ClosePanel,
    SplitBlock,
}

fn resolve_action(name: &str) -> Option<Action> {
    Some(match name {
        "save" => Action::Save,
        "save_as" => Action::SaveAs,
        "quit" => Action::Quit,
        "cancel" => Action::Cancel,
        "help" => Action::Help,
        "focus_next" => Action::FocusNext,
        "indent" => Action::Indent,
        "outdent" => Action::Outdent,
        "move_up" => Action::MoveUp,
        "move_down" => Action::MoveDown,
        "first_tab" => Action::FirstTab,
        "split_panel" => Action::SplitPanel,
        "close_panel" => Action::ClosePanel,
        "split_block" => Action::SplitBlock,
        _ => return None,
    })
}

/// Where in the scope chain the caller currently sits. Each context activates
/// itself and everything more global than it (`Block` activates all three).
#[derive(Clone, Copy, PartialEq)]
enum Context {
    Global,
    Panel,
    Block,
}

impl Context {
    fn name(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Panel => "panel",
            Context::Block => "block",
        }
    }

    /// F2 descends inward (more layers), wrapping back out to `global`.
    fn descend(self) -> Context {
        match self {
            Context::Global => Context::Panel,
            Context::Panel => Context::Block,
            Context::Block => Context::Global,
        }
    }
}

/// How a key was dispatched, for display. Distinct vocabularies so the reader
/// never confuses a layer hit with the sink, the reserved escape, or one of the
/// inspector's own control keys.
enum Dispatch<'a> {
    /// Resolved by a layer; `layer` is its name (the provenance).
    Hit {
        action: &'a Action,
        layer: &'static str,
    },
    /// No active layer bound it — falls through to the PTY sink.
    Sink,
    /// A reserved key pre-empted the chain entirely.
    Reserved,
    /// An inspector control key (F2/F3); not a binding, not passthrough.
    ToolControl(&'static str),
}

impl Dispatch<'_> {
    fn render(&self) -> String {
        match self {
            Dispatch::Hit { action, layer } => format!("{action:?} @ {layer}"),
            Dispatch::Sink => "→ PTY sink".to_string(),
            Dispatch::Reserved => "reserved (pre-empts all layers)".to_string(),
            Dispatch::ToolControl(label) => format!("(tui) {label}"),
        }
    }
}

/// First-hit over the ordered, *active* layers, returning **which** layer won
/// alongside the action — the provenance the bare `resolve_layered` discards.
///
/// This lives caller-side on purpose (the library's resolver stays
/// `Option<&A>`). It is a single scan, so the returned layer name and action
/// always come from the *same* binding — they cannot disagree by construction.
fn resolve_with_layer<'a, A>(
    layers: &[(&'static str, &'a Keymap<A>)],
    input: &KeyInput,
) -> Option<(&'static str, &'a A)> {
    layers
        .iter()
        .find_map(|(name, map)| map.get(input).map(|action| (*name, action)))
}

/// Is this the reserved escape hatch? Checked before resolution, so no binding
/// can shadow it.
fn is_reserved(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

struct LogRow {
    event: String,
    decoded: String,
    dispatch: String,
}

struct App {
    global: Keymap<Action>,
    panel: Keymap<Action>,
    block: Keymap<Action>,
    context: Context,
    log: Vec<LogRow>,
    enhancement_supported: bool,
    enhancement_on: bool,
    should_quit: bool,
}

impl App {
    /// The active layers for the current context, innermost first, each tagged
    /// with its name for provenance.
    fn active_layers(&self) -> Vec<(&'static str, &Keymap<Action>)> {
        let block = ("block", &self.block);
        let panel = ("panel", &self.panel);
        let global = ("global", &self.global);
        match self.context {
            Context::Global => vec![global],
            Context::Panel => vec![panel, global],
            Context::Block => vec![block, panel, global],
        }
    }

    /// All three layers in chain order with whether each is active in the
    /// current context — for the scope-chain diagram.
    fn chain(&self) -> [(&'static str, bool); 3] {
        let active = |c: Context| self.context as usize >= c as usize;
        // `Context` is ordered Global < Panel < Block by declaration; a layer is
        // active when the context is at least as deep as that layer.
        [
            ("block", matches!(self.context, Context::Block)),
            ("panel", active(Context::Panel)),
            ("global", true),
        ]
    }

    fn dispatch(&self, key: KeyEvent, decoded: Option<KeyInput>) -> Dispatch<'_> {
        if is_reserved(key) {
            return Dispatch::Reserved;
        }
        if let Some(label) = self.tool_control_label(key) {
            return Dispatch::ToolControl(label);
        }
        match decoded.and_then(|input| resolve_with_layer(&self.active_layers(), &input)) {
            Some((layer, action)) => Dispatch::Hit { action, layer },
            None => Dispatch::Sink,
        }
    }

    /// Labels the inspector's own control keys (F2/F3) so the log shows what they
    /// do rather than "→ PTY sink". F3 explains itself when the terminal lacks
    /// the kitty protocol, instead of looking like a dead key.
    fn tool_control_label(&self, key: KeyEvent) -> Option<&'static str> {
        match key.code {
            KeyCode::F(2) => Some("switch context"),
            KeyCode::F(3) if self.enhancement_supported => Some("toggle kitty protocol"),
            KeyCode::F(3) => Some("kitty protocol unsupported here"),
            _ => None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> io::Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        let decoded = KeyInput::try_from(key).ok();
        // Render to a String (and pull out the flags we act on) so the dispatch
        // — which borrows `self`'s keymaps — is dropped before we touch `&mut self`.
        let (dispatch_str, reserved, is_quit) = {
            let dispatch = self.dispatch(key, decoded);
            let reserved = matches!(dispatch, Dispatch::Reserved);
            let is_quit = matches!(
                dispatch,
                Dispatch::Hit {
                    action: Action::Quit,
                    ..
                }
            );
            (dispatch.render(), reserved, is_quit)
        };
        self.push_log(key, decoded, dispatch_str);

        // Reserved escape hatch: wins over every layer, can't be swallowed.
        if reserved {
            self.should_quit = true;
            return Ok(());
        }
        if is_quit {
            self.should_quit = true;
        }

        match key.code {
            KeyCode::F(2) => self.context = self.context.descend(),
            KeyCode::F(3) if self.enhancement_supported => {
                let next = !self.enhancement_on;
                set_enhancement(next)?;
                self.enhancement_on = next;
            }
            _ => {}
        }
        Ok(())
    }

    fn push_log(&mut self, key: KeyEvent, decoded: Option<KeyInput>, dispatch: String) {
        let decoded = decoded.map_or_else(|| "unsupported".to_string(), |input| input.to_string());
        // Full history is kept (printed on exit); the UI shows only the newest.
        self.log.insert(
            0,
            LogRow {
                event: format_event(key),
                decoded,
                dispatch,
            },
        );
    }
}

/// crossterm's raw view of the event, e.g. `ctrl+Char('i')`.
fn format_event(key: KeyEvent) -> String {
    let mut s = String::new();
    for (flag, name) in [
        (KeyModifiers::CONTROL, "ctrl"),
        (KeyModifiers::ALT, "alt"),
        (KeyModifiers::SHIFT, "shift"),
        (KeyModifiers::SUPER, "super"),
    ] {
        if key.modifiers.contains(flag) {
            s.push_str(name);
            s.push('+');
        }
    }
    let _ = write!(s, "{:?}", key.code);
    s
}

fn set_enhancement(on: bool) -> io::Result<()> {
    use crossterm::event::{
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    let mut out = io::stdout();
    if on {
        crossterm::execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
    } else {
        crossterm::execute!(out, PopKeyboardEnhancementFlags)
    }
}

fn build_keymap(toml: &str, what: &str) -> Keymap<Action> {
    let mut out: BuildOutput<Action> = keymap_config::from_str(toml, resolve_action)
        .unwrap_or_else(|_| panic!("valid {what} config"));
    out.layers
        .remove(keymap_config::GLOBAL_LAYER)
        .expect("the global layer is always present")
}

fn main() -> io::Result<()> {
    let mut app = App {
        global: build_keymap(GLOBAL_TOML, "global"),
        panel: build_keymap(PANEL_TOML, "panel"),
        block: build_keymap(BLOCK_TOML, "block"),
        context: Context::Block,
        log: Vec::new(),
        enhancement_supported: crossterm::terminal::supports_keyboard_enhancement()
            .unwrap_or(false),
        enhancement_on: false,
        should_quit: false,
    };

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);

    if app.enhancement_on {
        let _ = set_enhancement(false);
    }
    ratatui::restore();
    print_log(&app);
    result
}

/// After leaving the alternate screen, print the full session log to stdout so
/// it stays in the terminal scrollback for copying/sharing.
fn print_log(app: &App) {
    if app.log.is_empty() {
        return;
    }
    println!(
        "\nkeymap-tui — {} key event(s), oldest first:",
        app.log.len()
    );
    println!("{:<22}{:<16}dispatch", "crossterm event", "decoded");
    for row in app.log.iter().rev() {
        println!("{:<22}{:<16}{}", row.event, row.decoded, row.dispatch);
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui(frame, app))?;
        if let Event::Key(key) = event::read()? {
            app.handle_key(key)?;
        }
    }
    Ok(())
}

fn ui(frame: &mut Frame, app: &App) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_status(frame, areas[0], app);

    let panes = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(areas[1]);
    let left = Layout::vertical([Constraint::Length(9), Constraint::Min(0)]).split(panes[0]);
    render_scope_chain(frame, left[0], app);
    render_bindings(frame, left[1], app);
    render_log(frame, panes[1], app);

    let footer = Line::from(
        "F2: move context (global↔panel↔block)  |  F3: toggle kitty  |  ctrl+q: quit  |  ctrl+c: reserved force-quit",
    )
    .dim();
    frame.render_widget(Paragraph::new(footer), areas[2]);
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "?".to_string());
    let tmux = std::env::var("TMUX").is_ok();
    let ssh = std::env::var("SSH_TTY").is_ok() || std::env::var("SSH_CONNECTION").is_ok();

    let kitty = if app.enhancement_supported {
        format!(
            "kitty: supported=true active={}  (F3 toggles)",
            app.enhancement_on
        )
    } else {
        "kitty: supported=false  (F3 unavailable on this terminal)".to_string()
    };
    let lines = vec![Line::from(format!(
        "TERM={}  TERM_PROGRAM={}  tmux={tmux}  ssh={ssh}   {kitty}",
        env("TERM"),
        env("TERM_PROGRAM"),
    ))];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" keymap-tui inspector ")),
        area,
    );
}

/// The scope chain drawn top-to-bottom in resolution order: reserved (wins
/// over all) on top, then the layers (innermost first) with the active ones lit
/// and a `▶` on the innermost active, then the PTY sink past the end.
fn render_scope_chain(frame: &mut Frame, area: Rect, app: &App) {
    let chain = app.chain();
    let innermost_active = chain.iter().position(|(_, active)| *active);

    let mut lines = vec![
        Line::from("⚡ reserved   ctrl+c → force-quit  (wins over every layer)").yellow(),
        Line::from("── chain (first hit wins, miss falls outward) ──").dim(),
    ];
    for (i, (name, active)) in chain.iter().enumerate() {
        let marker = if Some(i) == innermost_active {
            "▶ "
        } else {
            "  "
        };
        let label = format!("{marker}{name}");
        let line = if *active {
            Line::from(format!("{label}    [active]"))
        } else {
            Line::from(format!("{label}    (inactive in this context)")).dim()
        };
        lines.push(line);
    }
    lines.push(Line::from("── sink (outside the chain) ──").dim());
    lines.push(Line::from("  PTY passthrough   (all layers miss → here)").dim());

    let title = format!(" scope chain — context: {} ", app.context.name());
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

/// All three layers' bindings, rendered from the real keymaps (no hard-coding),
/// so the reader can see which key is present/absent at each level — that is
/// what makes fall-through verifiable.
fn render_bindings(frame: &mut Frame, area: Rect, app: &App) {
    let mut items = Vec::new();
    for (name, map) in [
        ("block", &app.block),
        ("panel", &app.panel),
        ("global", &app.global),
    ] {
        items.push(ListItem::new(Line::from(format!("[{name}]")).bold()));
        let mut rows: Vec<(String, String)> = map
            .iter()
            .map(|(input, action)| (input.to_string(), format!("{action:?}")))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        if rows.is_empty() {
            items.push(ListItem::new(Line::from("    (no bindings)").dim()));
        }
        for (key, action) in rows {
            items.push(ListItem::new(format!("  {key:>12}  {action}")));
        }
    }

    frame.render_widget(
        List::new(items).block(Block::bordered().title(" layers (block / panel / global) ")),
        area,
    );
}

fn render_log(frame: &mut Frame, area: Rect, app: &App) {
    let header = ListItem::new(
        Line::from(format!(
            "{:<20}{:<14}{}",
            "crossterm event", "decoded", "dispatch"
        ))
        .bold(),
    );
    let rows = app.log.iter().take(MAX_LOG_ROWS).map(|row| {
        ListItem::new(format!(
            "{:<20}{:<14}{}",
            row.event, row.decoded, row.dispatch
        ))
    });
    let items: Vec<ListItem> = std::iter::once(header).chain(rows).collect();

    frame.render_widget(
        List::new(items).block(Block::bordered().title(" key log (newest first) ")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use keymap_core::{Key, Modifiers};

    fn ctrl(c: char) -> KeyInput {
        KeyInput::new(Key::Char(c), Modifiers::CTRL)
    }

    fn bound(toml: &str) -> Keymap<Action> {
        build_keymap(toml, "test")
    }

    // --- resolve_with_layer: pure, first-hit, provenance-carrying ---

    #[test]
    fn resolve_with_layer_picks_innermost_and_names_it() {
        let block = bound(BLOCK_TOML);
        let panel = bound(PANEL_TOML);
        let global = bound(GLOBAL_TOML);
        let layers = [("block", &block), ("panel", &panel), ("global", &global)];
        // ctrl+s is in all three; innermost (block) wins, and the name matches.
        assert_eq!(
            resolve_with_layer(&layers, &ctrl('s')),
            Some(("block", &Action::SplitBlock))
        );
    }

    #[test]
    fn resolve_with_layer_falls_through_to_panel() {
        let block = bound(BLOCK_TOML);
        let panel = bound(PANEL_TOML);
        let global = bound(GLOBAL_TOML);
        let layers = [("block", &block), ("panel", &panel), ("global", &global)];
        // ctrl+w is panel-only: misses block, hits panel.
        assert_eq!(
            resolve_with_layer(&layers, &ctrl('w')),
            Some(("panel", &Action::ClosePanel))
        );
    }

    #[test]
    fn resolve_with_layer_skips_middle_to_global() {
        let block = bound(BLOCK_TOML);
        let panel = bound(PANEL_TOML);
        let global = bound(GLOBAL_TOML);
        let layers = [("block", &block), ("panel", &panel), ("global", &global)];
        // ctrl+q is global-only: misses block and panel, resolves at global.
        assert_eq!(
            resolve_with_layer(&layers, &ctrl('q')),
            Some(("global", &Action::Quit))
        );
    }

    #[test]
    fn resolve_with_layer_misses_when_unbound_or_empty() {
        let global = bound(GLOBAL_TOML);
        let layers = [("global", &global)];
        assert_eq!(resolve_with_layer(&layers, &ctrl('z')), None);
        // Empty layer list is a miss, not a panic.
        let none: [(&'static str, &Keymap<Action>); 0] = [];
        assert_eq!(resolve_with_layer(&none, &ctrl('s')), None);
    }

    #[test]
    fn resolve_with_layer_skips_empty_layers_without_index_drift() {
        let empty = Keymap::<Action>::new();
        let global = bound(GLOBAL_TOML);
        // An empty inner layer must not steal provenance from the layer that hits.
        let layers = [("block", &empty), ("panel", &empty), ("global", &global)];
        assert_eq!(
            resolve_with_layer(&layers, &ctrl('s')),
            Some(("global", &Action::Save))
        );
    }

    // --- demo config integrity: the flow can actually exhibit (a)-(e) ---

    #[test]
    fn demo_layers_can_exhibit_the_whole_checklist() {
        let block = bound(BLOCK_TOML);
        let panel = bound(PANEL_TOML);
        let global = bound(GLOBAL_TOML);

        // (a) override: ctrl+s present in all three with *distinct* actions.
        let s = ctrl('s');
        let (b, p, g) = (block.get(&s), panel.get(&s), global.get(&s));
        assert!(b.is_some() && p.is_some() && g.is_some());
        assert!(
            b != p && p != g && b != g,
            "ctrl+s must differ per layer to show provenance"
        );

        // (b) block-misses-panel-hits: ctrl+w in panel only.
        assert!(block.get(&ctrl('w')).is_none() && panel.get(&ctrl('w')).is_some());

        // (c)/(f) middle-skip: ctrl+q in global only.
        assert!(block.get(&ctrl('q')).is_none() && panel.get(&ctrl('q')).is_none());
        assert!(global.get(&ctrl('q')).is_some());

        // (d) sink: ctrl+z bound nowhere.
        assert!(block.get(&ctrl('z')).is_none());
        assert!(panel.get(&ctrl('z')).is_none());
        assert!(global.get(&ctrl('z')).is_none());

        // (e) reserved pre-emption is observable: ctrl+c IS bound (global), yet
        // `is_reserved` short-circuits before resolution.
        assert!(global.get(&ctrl('c')).is_some());
    }

    #[test]
    fn block_layer_is_not_degenerate() {
        // Guard against a future edit emptying block, which would make the
        // innermost-override demonstration impossible.
        assert!(!bound(BLOCK_TOML).is_empty());
    }

    #[test]
    fn reserved_key_is_detected() {
        let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_reserved(ev));
        let not = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(!is_reserved(not));
    }

    #[test]
    fn tool_control_label_explains_f2_f3_including_unsupported_kitty() {
        let mut app = App {
            global: bound(GLOBAL_TOML),
            panel: bound(PANEL_TOML),
            block: bound(BLOCK_TOML),
            context: Context::Block,
            log: Vec::new(),
            enhancement_supported: false,
            enhancement_on: false,
            should_quit: false,
        };
        let f2 = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        let f3 = KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE);
        assert_eq!(app.tool_control_label(f2), Some("switch context"));
        // Unsupported terminal: F3 explains itself rather than reading as a miss.
        assert_eq!(
            app.tool_control_label(f3),
            Some("kitty protocol unsupported here")
        );
        app.enhancement_supported = true;
        assert_eq!(app.tool_control_label(f3), Some("toggle kitty protocol"));
        // A normal key is not a tool control.
        let s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(app.tool_control_label(s), None);
    }

    #[test]
    fn context_descend_cycles_inward_then_wraps() {
        assert!(matches!(Context::Global.descend(), Context::Panel));
        assert!(matches!(Context::Panel.descend(), Context::Block));
        assert!(matches!(Context::Block.descend(), Context::Global));
    }

    #[test]
    fn chain_activation_tracks_context() {
        let mk = |context| App {
            global: bound(GLOBAL_TOML),
            panel: bound(PANEL_TOML),
            block: bound(BLOCK_TOML),
            context,
            log: Vec::new(),
            enhancement_supported: false,
            enhancement_on: false,
            should_quit: false,
        };
        // global context: only global active.
        assert_eq!(
            mk(Context::Global).chain(),
            [("block", false), ("panel", false), ("global", true)]
        );
        // panel context: panel + global.
        assert_eq!(
            mk(Context::Panel).chain(),
            [("block", false), ("panel", true), ("global", true)]
        );
        // block context: all three.
        assert_eq!(
            mk(Context::Block).chain(),
            [("block", true), ("panel", true), ("global", true)]
        );
    }
}
