// trellis/src/tui/rename.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::manager::Manager;
use crate::tmux;
use super::{ScreenAction, ScreenBehavior};
use super::theme;

// ---------------------------------------------------------------------------
// Shared text input helpers
// ---------------------------------------------------------------------------

pub fn input_handle_key(input: &mut String, cursor: &mut usize, code: KeyCode, modifiers: KeyModifiers) {
    input_handle_key_ex(input, cursor, code, modifiers, false);
}

/// Like `input_handle_key`, plus optional multi-line support. When
/// `multiline` is true: Enter with any modifier inserts `\n` at the cursor,
/// and Up/Down jump the cursor to the same column on the adjacent line.
/// Plain Enter (no modifiers) is still ignored here — callers handle save.
pub fn input_handle_key_ex(
    input: &mut String,
    cursor: &mut usize,
    code: KeyCode,
    modifiers: KeyModifiers,
    multiline: bool,
) {
    if multiline {
        // Newline insertion triggers (any of these works, so the binding
        // survives terminals that intercept one or another):
        //   - Enter with any modifier (Shift/Alt/Ctrl) — e.g. Shift+Enter
        //   - Ctrl+J — the classic Unix "literal newline", almost never
        //     intercepted by terminal emulators
        let is_newline_insert = matches!(code, KeyCode::Enter) && !modifiers.is_empty()
            || matches!(code, KeyCode::Char('j')) && modifiers.contains(KeyModifiers::CONTROL);
        if is_newline_insert {
            input.insert(*cursor, '\n');
            *cursor += 1;
            return;
        }
        if matches!(code, KeyCode::Up | KeyCode::Down) {
            jump_line(input, cursor, code == KeyCode::Up);
            return;
        }
    }
    match code {
        KeyCode::Char(c) => {
            // Ctrl+U: clear line
            if modifiers.contains(KeyModifiers::CONTROL) && c == 'u' {
                input.clear();
                *cursor = 0;
            } else if modifiers.contains(KeyModifiers::CONTROL) && c == 'w' {
                // Ctrl+W: delete word before cursor
                let s = &input[..*cursor];
                let new_end = s.trim_end().rfind(' ').map(|i| i + 1).unwrap_or(0);
                input.drain(new_end..*cursor);
                *cursor = new_end;
            } else {
                input.insert(*cursor, c);
                *cursor += c.len_utf8();
            }
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                let prev = input[..*cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                input.drain(prev..*cursor);
                *cursor = prev;
            }
        }
        KeyCode::Delete => {
            if *cursor < input.len() {
                let next = input[*cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| *cursor + i)
                    .unwrap_or(input.len());
                input.drain(*cursor..next);
            }
        }
        KeyCode::Left => {
            if *cursor > 0 {
                let prev = input[..*cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                *cursor = prev;
            }
        }
        KeyCode::Right => {
            if *cursor < input.len() {
                let next = input[*cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| *cursor + i)
                    .unwrap_or(input.len());
                *cursor = next;
            }
        }
        KeyCode::Home => {
            *cursor = 0;
        }
        KeyCode::End => {
            *cursor = input.len();
        }
        _ => {}
    }
}

fn jump_line(input: &str, cursor: &mut usize, up: bool) {
    // Find start of current line and column within it.
    let cur = *cursor;
    let line_start = input[..cur].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = cur - line_start;

    if up {
        // Find start of previous line, if any.
        if line_start == 0 {
            return;
        }
        let prev_line_end = line_start - 1; // position of the `\n`
        let prev_line_start = input[..prev_line_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prev_line_len = prev_line_end - prev_line_start;
        *cursor = prev_line_start + col.min(prev_line_len);
    } else {
        // Find end of current line (next `\n`) and start of next.
        let cur_line_end = match input[cur..].find('\n') {
            Some(rel) => cur + rel,
            None => return, // no next line
        };
        let next_line_start = cur_line_end + 1;
        let next_line_end = input[next_line_start..]
            .find('\n')
            .map(|r| next_line_start + r)
            .unwrap_or(input.len());
        let next_line_len = next_line_end - next_line_start;
        *cursor = next_line_start + col.min(next_line_len);
    }
}

/// Render a text input line with cursor highlighting into the given area.
pub fn render_input(f: &mut Frame, area: Rect, input: &str, cursor_pos: usize) {
    let cursor_char_len = if cursor_pos < input.len() {
        input[cursor_pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0)
    } else {
        0
    };
    let after = if cursor_pos + cursor_char_len <= input.len() {
        &input[cursor_pos + cursor_char_len..]
    } else {
        ""
    };
    let cursor_display = if cursor_pos < input.len() {
        input[cursor_pos..cursor_pos + cursor_char_len].to_string()
    } else {
        " ".to_string()
    };

    let line = Line::from(vec![
        Span::styled(input[..cursor_pos].to_string(), Style::default().fg(theme::TEXT)),
        Span::styled(cursor_display, theme::style_cursor()),
        Span::styled(after.to_string(), Style::default().fg(theme::TEXT)),
    ]);
    let para = Paragraph::new(line);
    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn render_input_actually_emits_cursor_span_at_end() {
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| {
            let area = f.area();
            render_input(f, area, "abc", 3);
        }).unwrap();
        let buf = terminal.backend().buffer();
        // Cursor should be at column 3 (the space after "abc").
        let cursor_cell = &buf[(3u16, 0u16)];
        let bg_matches_cursor = cursor_cell.bg == theme::CURSOR_BG;
        // Cells before the cursor should have TEXT fg.
        let pre_cell = &buf[(0u16, 0u16)];
        assert_eq!(pre_cell.symbol(), "a");
        assert!(bg_matches_cursor, "cursor cell at col 3 has bg {:?}, expected {:?}; symbol: {:?}",
            cursor_cell.bg, theme::CURSOR_BG, cursor_cell.symbol());
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|sp| sp.content.as_ref()).collect()
    }

    #[test]
    fn footer_lines_keep_every_binding_whole_and_within_width() {
        let bindings = [("q", "Quit"), ("enter", "Switch"), ("h/l", "Collapse/Expand"), ("S", "Settings")];
        for width in [20u16, 30, 45, 200] {
            let lines = footer_lines(&bindings, width);
            let joined: Vec<String> = lines.iter().map(line_text).collect();
            for line in &joined {
                assert!(line.chars().count() <= width as usize, "{:?} exceeds {}", line, width);
            }
            let all = joined.join("  ");
            let mut at = 0;
            for (k, d) in &bindings {
                let pair = format!("{} {}", k, d);
                at += all[at..].find(&pair).expect("binding split or missing");
            }
        }
    }

    #[test]
    fn footer_lines_use_one_line_when_everything_fits() {
        assert_eq!(footer_lines(&[("q", "Quit"), ("?", "Help")], 80).len(), 1);
        assert!(footer_lines(&[("q", "Quit"), ("?", "Help")], 10).len() > 1);
    }
}

/// Render the centered modal box and return the inner area.
pub fn render_modal_box(f: &mut Frame, area: Rect, title: &str, width: u16, height: u16) -> Rect {
    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .split(area);

    let horiz = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .split(vert[1]);

    let box_area = horiz[1];

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .style(Style::default().fg(theme::TEXT).bg(theme::HEADER_BG));

    let inner = block.inner(box_area);
    f.render_widget(block, box_area);
    inner
}

/// Render a footer bar from key-description pairs with consistent styling.
pub fn render_footer_bindings<'a>(bindings: &[(&'a str, &'a str)]) -> Paragraph<'a> {
    let mut spans = Vec::new();
    for (i, (key, desc)) in bindings.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", theme::style_footer()));
        }
        spans.push(Span::styled(*key, theme::style_footer_key()));
        spans.push(Span::styled(format!(" {}", desc), theme::style_footer()));
    }
    Paragraph::new(Line::from(spans)).style(theme::style_footer())
}

/// Pack footer bindings into as many lines as `width` needs, never splitting a
/// `key desc` pair across lines.
pub fn footer_lines<'a>(bindings: &[(&'a str, &'a str)], width: u16) -> Vec<Line<'a>> {
    let width = width.max(1) as usize;
    let mut lines: Vec<Vec<Span<'a>>> = vec![Vec::new()];
    let mut used = 0;
    for (key, desc) in bindings {
        let item_width = key.chars().count() + 1 + desc.chars().count();
        if used > 0 && used + 2 + item_width > width {
            lines.push(Vec::new());
            used = 0;
        }
        let line = lines.last_mut().expect("lines starts non-empty");
        if used > 0 {
            line.push(Span::styled("  ", theme::style_footer()));
            used += 2;
        }
        line.push(Span::styled(*key, theme::style_footer_key()));
        line.push(Span::styled(format!(" {}", desc), theme::style_footer()));
        used += item_width;
    }
    lines.into_iter().map(Line::from).collect()
}

// ---------------------------------------------------------------------------
// RenameSessionScreen
// ---------------------------------------------------------------------------

pub struct RenameSessionScreen {
    session_id: i64,
    current_name: String,
    input: String,
    cursor_pos: usize,
    error: String,
}

impl RenameSessionScreen {
    pub fn new(_manager: &Manager, session_id: i64, current_name: String) -> Self {
        let cursor_pos = current_name.len();
        let input = current_name.clone();
        Self { session_id, current_name, input, cursor_pos, error: String::new() }
    }
}

impl ScreenBehavior for RenameSessionScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let title = format!("Rename Session — {}", self.current_name);
        let inner = render_modal_box(f, area, &title, 74, 9);

        let chunks = Layout::vertical([
            Constraint::Length(1), // input label
            Constraint::Length(1), // input
            Constraint::Length(1), // error
            Constraint::Length(1), // hint
        ])
        .split(inner);

        // Input label
        let label = Paragraph::new(Span::styled("New name:", Style::default().fg(theme::TEXT_DIM)));
        f.render_widget(label, chunks[0]);

        // Text input with cursor
        render_input(f, chunks[1], &self.input, self.cursor_pos);

        // Error
        if !self.error.is_empty() {
            let err = Paragraph::new(Span::styled(self.error.clone(), theme::style_error()));
            f.render_widget(err, chunks[2]);
        }

        // Hint
        let hint = Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme::ACCENT)),
            Span::styled(" to rename  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Escape", Style::default().fg(theme::ACCENT)),
            Span::styled(" to cancel", Style::default().fg(theme::TEXT_DIM)),
        ]);
        let hint_para = Paragraph::new(hint).alignment(Alignment::Center);
        f.render_widget(hint_para, chunks[3]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => return ScreenAction::Pop,
                KeyCode::Enter => {
                    let name = tmux::sanitize_session_name(self.input.trim());
                    if name.is_empty() {
                        self.error = "Name cannot be empty.".to_string();
                        return ScreenAction::None;
                    }
                    if name == self.current_name {
                        return ScreenAction::Pop;
                    }
                    if manager.get_session_by_name(&name).is_some() {
                        self.error = format!("Session '{}' already exists.", name);
                        return ScreenAction::None;
                    }
                    match manager.rename_session(self.session_id, &name) {
                        Ok(_) => return ScreenAction::Pop,
                        Err(e) => {
                            self.error = e;
                            return ScreenAction::None;
                        }
                    }
                }
                _ => {
                    input_handle_key(&mut self.input, &mut self.cursor_pos, *code, *modifiers);
                    self.error.clear();
                }
            }
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// RenameWindowScreen
// ---------------------------------------------------------------------------

pub struct RenameWindowScreen {
    session_name: String,
    window_index: i64,
    current_name: String,
    input: String,
    cursor_pos: usize,
    error: String,
}

impl RenameWindowScreen {
    pub fn new(session_name: String, window_index: i64, current_name: String) -> Self {
        let cursor_pos = current_name.len();
        let input = current_name.clone();
        Self { session_name, window_index, current_name, input, cursor_pos, error: String::new() }
    }
}

impl ScreenBehavior for RenameWindowScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let title = format!("Rename Tab — {}:{}", self.session_name, self.current_name);
        let inner = render_modal_box(f, area, &title, 74, 9);

        let chunks = Layout::vertical([
            Constraint::Length(1), // input label
            Constraint::Length(1), // input
            Constraint::Length(1), // error
            Constraint::Length(1), // hint
        ])
        .split(inner);

        let label = Paragraph::new(Span::styled("New name:", Style::default().fg(theme::TEXT_DIM)));
        f.render_widget(label, chunks[0]);

        render_input(f, chunks[1], &self.input, self.cursor_pos);

        if !self.error.is_empty() {
            let err = Paragraph::new(Span::styled(self.error.clone(), theme::style_error()));
            f.render_widget(err, chunks[2]);
        }

        let hint = Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme::ACCENT)),
            Span::styled(" to rename  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Escape", Style::default().fg(theme::ACCENT)),
            Span::styled(" to cancel", Style::default().fg(theme::TEXT_DIM)),
        ]);
        let hint_para = Paragraph::new(hint).alignment(Alignment::Center);
        f.render_widget(hint_para, chunks[3]);
    }

    fn handle_event(&mut self, event: &Event, _manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => return ScreenAction::Pop,
                KeyCode::Enter => {
                    let name = self.input.trim().to_string();
                    if name.is_empty() {
                        self.error = "Name cannot be empty.".to_string();
                        return ScreenAction::None;
                    }
                    if name == self.current_name {
                        return ScreenAction::Pop;
                    }
                    match tmux::rename_window(&self.session_name, self.window_index, &name) {
                        Ok(_) => return ScreenAction::Pop,
                        Err(e) => {
                            self.error = e.to_string();
                            return ScreenAction::None;
                        }
                    }
                }
                _ => {
                    input_handle_key(&mut self.input, &mut self.cursor_pos, *code, *modifiers);
                    self.error.clear();
                }
            }
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        true
    }
}
