// trellis/src/tui/settings.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::db;
use crate::manager::Manager;
use super::{ScreenAction, ScreenBehavior};
use super::theme;

const CONFIG_KEYS: &[(&str, &str)] = &[
    ("repos_dir", "Repos directory"),
    ("worktrees_dir", "Worktrees directory"),
];

struct Field {
    key: String,
    label: String,
    value: String,
    cursor: usize,
}

impl Field {
    fn new(key: &str, label: &str, value: &str) -> Self {
        let cursor = value.len();
        Self {
            key: key.to_string(),
            label: label.to_string(),
            value: value.to_string(),
            cursor,
        }
    }
}

pub struct SettingsScreen {
    fields: Vec<Field>,
    focused: usize,
    status: String,
    status_is_error: bool,
}

impl SettingsScreen {
    pub fn new(manager: &Manager) -> Self {
        let config: std::collections::HashMap<String, String> =
            db::get_all_config(&manager.conn).into_iter().collect();

        let fields = CONFIG_KEYS
            .iter()
            .map(|(key, label)| {
                let value = config.get(*key).cloned().unwrap_or_default();
                Field::new(key, label, &value)
            })
            .collect();

        Self {
            fields,
            focused: 0,
            status: String::new(),
            status_is_error: false,
        }
    }

    fn handle_field_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let field = &mut self.fields[self.focused];
        super::rename::input_handle_key(&mut field.value, &mut field.cursor, code, modifiers);
    }

    fn save(&mut self, manager: &mut Manager) {
        for field in &self.fields {
            let value = field.value.trim();
            if value.is_empty() {
                self.status = format!("{} cannot be empty.", field.label);
                self.status_is_error = true;
                self.focused = self.fields.iter().position(|f| f.key == field.key).unwrap_or(0);
                return;
            }
        }
        for field in &self.fields {
            db::set_config(&manager.conn, &field.key, field.value.trim());
        }
        self.status = "Saved.".to_string();
        self.status_is_error = false;
    }
}

impl ScreenBehavior for SettingsScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let width = 82u16;
        let field_rows = self.fields.len() as u16 * 2;
        let height = (field_rows + 8).min(area.height.saturating_sub(4));
        let inner = super::rename::render_modal_box(f, area, "Settings", width, height);

        // Build constraints: hint, gap, then per-field: label + input, status, footer hint
        let mut constraints = vec![
            Constraint::Length(1), // hint
            Constraint::Length(1), // gap
        ];
        for _ in &self.fields {
            constraints.push(Constraint::Length(1)); // label
            constraints.push(Constraint::Length(1)); // input
            constraints.push(Constraint::Length(1)); // gap between fields
        }
        constraints.push(Constraint::Length(1)); // status
        constraints.push(Constraint::Length(1)); // footer hint

        let chunks = Layout::vertical(constraints).split(inner);

        // Hint
        let hint = Paragraph::new(Span::styled(
            "Edit values and press Enter to save.",
            Style::default().fg(theme::TEXT_DIM),
        ))
        .alignment(Alignment::Center);
        f.render_widget(hint, chunks[0]);

        // Fields
        let mut chunk_idx = 2usize;
        for (i, field) in self.fields.iter().enumerate() {
            let is_focused = i == self.focused;

            // Label
            let label_style = if is_focused {
                Style::default().fg(theme::ACCENT)
            } else {
                Style::default().fg(theme::TEXT_DIM)
            };
            let label = Paragraph::new(Span::styled(format!("{}:", field.label), label_style));
            f.render_widget(label, chunks[chunk_idx]);
            chunk_idx += 1;

            // Input with cursor (only show cursor for focused field)
            if is_focused {
                super::rename::render_input(f, chunks[chunk_idx], &field.value, field.cursor);
            } else {
                let line = Line::from(Span::styled(field.value.clone(), Style::default().fg(theme::TEXT_DIM)));
                f.render_widget(Paragraph::new(line), chunks[chunk_idx]);
            }
            chunk_idx += 1;
            chunk_idx += 1; // gap
        }

        // Status line
        if !self.status.is_empty() {
            let style = if self.status_is_error {
                theme::style_error()
            } else {
                theme::style_green()
            };
            let status = Paragraph::new(Span::styled(self.status.clone(), style))
                .alignment(Alignment::Center);
            f.render_widget(status, chunks[chunk_idx]);
        }
        chunk_idx += 1;

        // Footer hint
        let footer = Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme::ACCENT)),
            Span::styled(" to save  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Tab", Style::default().fg(theme::ACCENT)),
            Span::styled(" to switch field  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Escape", Style::default().fg(theme::ACCENT)),
            Span::styled(" to cancel", Style::default().fg(theme::TEXT_DIM)),
        ]);
        let footer_para = Paragraph::new(footer).alignment(Alignment::Center);
        f.render_widget(footer_para, chunks[chunk_idx]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => return ScreenAction::Pop,
                KeyCode::Enter => {
                    self.save(manager);
                    return ScreenAction::None;
                }
                KeyCode::Tab => {
                    if !self.fields.is_empty() {
                        self.focused = (self.focused + 1) % self.fields.len();
                    }
                    self.status.clear();
                }
                KeyCode::BackTab => {
                    if !self.fields.is_empty() {
                        if self.focused == 0 {
                            self.focused = self.fields.len() - 1;
                        } else {
                            self.focused -= 1;
                        }
                    }
                    self.status.clear();
                }
                _ => {
                    if !self.fields.is_empty() {
                        self.handle_field_key(*code, *modifiers);
                        self.status.clear();
                    }
                }
            }
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        true
    }
}
