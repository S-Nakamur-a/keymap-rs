//! `keymap-showcase` — interactive 5-value demonstration TUI.
//!
//! Run with:
//! ```sh
//! cargo run -p keymap-showcase
//! ```
//!
//! Controls:
//! - Normal mode: `ctrl+s` save, `ctrl+q` quit, `j`/`k` move, `g g` jump top.
//! - `:` enters the command palette; type a prefix and Enter to dispatch.
//! - `F5` starts rebind capture for `ctrl+s`; press any non-reserved key.
//! - `F6` deletes the `ctrl+z` binding (tombstone demo).
//! - `ctrl+c` is reserved and always force-quits.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use keymap_showcase::{AppState, Mode, Outcome, default_save_path};
use keymap_suite::{Key, KeyInput, Modifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Stylize,
    text::Line,
    widgets::{Block, List, ListItem, Paragraph},
};

const SEQ_WINDOW: Duration = Duration::from_millis(500);

fn main() -> io::Result<()> {
    // If $HOME is not set, fall back to a temp-dir path so saves do not land in
    // a PTY-writable current working directory. default_save_path() returns None
    // in that case and we substitute a safe temp path.
    let save_path = default_save_path()
        .unwrap_or_else(|| std::env::temp_dir().join("keymap-showcase-user.toml"));
    let mut app = AppState::new(save_path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut AppState) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;
        if app.should_quit {
            break;
        }

        // Value 4: use TimedPending::deadline for the poll timeout.
        let timeout = app
            .pending
            .deadline(SEQ_WINDOW)
            .map_or(Duration::from_secs(10), |d| {
                d.saturating_duration_since(Instant::now())
            });

        if !event::poll(timeout)? {
            // Timeout fired — flush the pending sequence buffer.
            let flushed = app.pending.flush();
            if !flushed.is_empty() {
                app.status = format!("sequence timeout, {} key(s) flushed", flushed.len());
            }
            continue;
        }

        if let Event::Key(key) = event::read()? {
            handle_key(app, key);
        }
    }
    Ok(())
}

fn handle_key(app: &mut AppState, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    // ctrl+c is reserved — always quits, bypasses all layers.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    // F5 = start rebind for ctrl+s.
    if key.code == KeyCode::F(5) {
        app.start_rebind("ctrl+s".to_string());
        return;
    }

    // F6 = tombstone ctrl+z.
    if key.code == KeyCode::F(6) {
        let z = KeyInput::new(Key::Char('z'), Modifiers::CTRL);
        app.delete_binding(&z);
        return;
    }

    // F7 = save config.
    if key.code == KeyCode::F(7) {
        match app.save_config() {
            Ok(()) => app.status = "config saved".to_string(),
            Err(e) => app.status = format!("save error: {e}"),
        }
        return;
    }

    let Ok(input) = KeyInput::try_from(key) else {
        return;
    };

    let outcome = app.feed(input, Instant::now(), SEQ_WINDOW);
    if matches!(outcome, Outcome::Quit) {
        app.should_quit = true;
    }
}

fn ui(frame: &mut Frame, app: &AppState) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_mode_status(frame, areas[0], app);
    render_main(frame, areas[1], app);
    render_lints_and_decode(frame, areas[2], app);
    render_footer(frame, areas[3]);
}

fn render_mode_status(frame: &mut Frame, area: ratatui::layout::Rect, app: &AppState) {
    let mode_label = match &app.mode {
        Mode::Normal => "NORMAL".to_string(),
        Mode::CaptureRebind { chord } => format!("REBIND ← {chord}"),
        Mode::Palette { text } => format!("PALETTE  :{text}▌"),
    };

    let lines = vec![
        Line::from(format!(" mode: {mode_label}")),
        Line::from(format!(" status: {}", app.status)).dim(),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" keymap-showcase ")),
        area,
    );
}

fn render_main(frame: &mut Frame, area: ratatui::layout::Rect, app: &AppState) {
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    // Left: bindings + merge notes.
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from("global layer:").bold())];
    let mut rows: Vec<(String, String)> = app
        .global
        .iter()
        .map(|(k, a)| (k.to_string(), format!("{a:?}")))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (chord, action) in &rows {
        items.push(ListItem::new(format!("  {chord:>12}  {action}")));
    }
    if !app.merge_notes.is_empty() {
        items.push(ListItem::new(
            Line::from(format!("\nmerge notes ({})", app.merge_notes.len())).dim(),
        ));
        for note in &app.merge_notes {
            items.push(ListItem::new(Line::from(format!("  {note:?}")).dim()));
        }
    }
    frame.render_widget(
        List::new(items).block(Block::bordered().title(" bindings + merge notes ")),
        cols[0],
    );

    // Right: value 4 which-key + value 5 palette + warnings.
    let mut right_lines: Vec<Line> = Vec::new();
    right_lines.push(Line::from("sequences pending:").bold());
    let conts = app.continuations();
    if conts.is_empty() {
        right_lines.push(Line::from("  (none)").dim());
    } else {
        right_lines.push(Line::from(format!("  next keys: {}", conts.join(", "))));
    }

    right_lines.push(Line::from(""));
    right_lines.push(Line::from("palette candidates:").bold());
    let candidates = app.palette_candidates();
    if candidates.is_empty() {
        right_lines.push(Line::from("  (enter palette with ':')").dim());
    } else {
        for (name, _) in candidates.iter().take(6) {
            right_lines.push(Line::from(format!("  {name}")));
        }
    }

    if !app.warnings.is_empty() {
        right_lines.push(Line::from(""));
        right_lines.push(Line::from(format!("warnings ({}):", app.warnings.len())).yellow());
        for w in app.warnings.iter().take(4) {
            right_lines.push(Line::from(format!("  {w}")).yellow());
        }
    }

    frame.render_widget(
        Paragraph::new(right_lines)
            .block(Block::bordered().title(" sequences / palette / warnings ")),
        cols[1],
    );
}

fn render_lints_and_decode(frame: &mut Frame, area: ratatui::layout::Rect, app: &AppState) {
    let cols =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area);

    let mut lint_lines: Vec<Line> =
        vec![Line::from("legacy lints (keymap_suite::legacy_lints):").bold()];
    if app.legacy_warnings.is_empty() {
        lint_lines.push(Line::from("  all chords survive legacy terminals").dim());
    } else {
        for lw in app.legacy_warnings.iter().take(4) {
            lint_lines.push(Line::from(format!("  {}  {}", lw.chord, lw.detail)).yellow());
        }
    }
    frame.render_widget(
        Paragraph::new(lint_lines)
            .block(Block::bordered().title(" value 3: terminal survivability ")),
        cols[0],
    );

    let decode_lines = vec![
        Line::from("canned-bytes decode (keymap-term):").bold(),
        Line::from(format!("  {}", app.decode_demo)).dim(),
    ];
    frame.render_widget(
        Paragraph::new(decode_lines).block(Block::bordered().title(" decode demo ")),
        cols[1],
    );
}

fn render_footer(frame: &mut Frame, area: ratatui::layout::Rect) {
    let footer = Line::from(
        "ctrl+q: quit  |  :: palette  |  F5: rebind ctrl+s  |  F6: delete ctrl+z  |  F7: save  |  ctrl+c: reserved",
    )
    .dim();
    frame.render_widget(Paragraph::new(footer), area);
}
