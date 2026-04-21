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
