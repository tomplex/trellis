// trellis/src/tui/settings.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::manager::Manager;
use super::{ScreenAction, ScreenBehavior};
use super::theme;

const CONFIG_KEYS: &[(&str, &str, bool)] = &[
    // (key, label, multiline)
    ("repos_dir", "Repos directories (one path per line)", true),
    ("worktrees_dir", "Worktrees directory", false),
    ("agent_cmd", "Agent command (pi, omp, claude)", false),
    (crate::notify::CONFIG_KEY, "Ping when an agent needs input (on/off)", false),
    (crate::notify::AI_CONFIG_KEY, "Ask Haiku whether a reply needs input (on/off)", false),
];

struct Field {
    key: String,
    label: String,
    value: String,
    cursor: usize,
    multiline: bool,
}

impl Field {
    fn new(key: &str, label: &str, value: &str, multiline: bool) -> Self {
        let cursor = value.len();
        Self {
            key: key.to_string(),
            label: label.to_string(),
            value: value.to_string(),
            cursor,
            multiline,
        }
    }

    fn line_count(&self) -> usize {
        if self.multiline {
            self.value.lines().count().max(1)
        } else {
            1
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
            manager.get_all_config().into_iter().collect();

        let fields = CONFIG_KEYS
            .iter()
            .map(|(key, label, multiline)| {
                let value = config.get(*key).cloned().unwrap_or_default();
                Field::new(key, label, &value, *multiline)
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
        super::rename::input_handle_key_ex(
            &mut field.value,
            &mut field.cursor,
            code,
            modifiers,
            field.multiline,
        );
    }

    /// Clean a field's value before saving: trim each line, drop blank lines.
    /// For single-line fields this collapses to the existing trim.
    fn cleaned(field: &Field) -> String {
        if field.multiline {
            field
                .value
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            field.value.trim().to_string()
        }
    }

    fn save(&mut self, manager: &mut Manager) {
        for field in &self.fields {
            let value = Self::cleaned(field);
            if value.is_empty() {
                self.status = format!("{} cannot be empty.", field.label);
                self.status_is_error = true;
                self.focused = self.fields.iter().position(|f| f.key == field.key).unwrap_or(0);
                return;
            }
        }
        for field in &self.fields {
            manager.set_config(&field.key, &Self::cleaned(field));
        }
        self.status = "Saved.".to_string();
        self.status_is_error = false;
    }
}

impl ScreenBehavior for SettingsScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let width = 82u16;
        // Each field: 1 label + N input lines + 1 gap. Plus 4 lines of chrome
        // (hint + gap + status + footer) and 2 lines for the modal's borders.
        let field_rows: u16 = self
            .fields
            .iter()
            .map(|f| 1 + f.line_count() as u16 + 1)
            .sum();
        let height = (field_rows + 6).min(area.height.saturating_sub(4));
        let inner = super::rename::render_modal_box(f, area, "Settings", width, height);

        // Build constraints: hint, gap, then per-field: label + input(s) + gap, status, footer hint
        let mut constraints = vec![
            Constraint::Length(1), // hint
            Constraint::Length(1), // gap
        ];
        for field in &self.fields {
            constraints.push(Constraint::Length(1)); // label
            constraints.push(Constraint::Length(field.line_count() as u16)); // input(s)
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

            // Input with cursor (only show cursor for focused field).
            // Multi-line fields split the chunk into one row per line and
            // route the cursor render to the line that contains it.
            let input_area = chunks[chunk_idx];
            if field.multiline {
                let line_count = field.line_count();
                let input_constraints: Vec<_> = (0..line_count).map(|_| Constraint::Length(1)).collect();
                let line_chunks = Layout::vertical(input_constraints).split(input_area);
                // Identify cursor line + column.
                let (cur_line_idx, cur_col) = line_offset(&field.value, field.cursor);
                for (i, line) in field.value.split('\n').enumerate() {
                    if i >= line_chunks.len() {
                        break;
                    }
                    if is_focused && i == cur_line_idx {
                        super::rename::render_input(f, line_chunks[i], line, cur_col);
                    } else {
                        let style = if is_focused {
                            Style::default()
                        } else {
                            Style::default().fg(theme::TEXT_DIM)
                        };
                        let p = Paragraph::new(Line::from(Span::styled(line.to_string(), style)));
                        f.render_widget(p, line_chunks[i]);
                    }
                }
            } else if is_focused {
                super::rename::render_input(f, input_area, &field.value, field.cursor);
            } else {
                let line = Line::from(Span::styled(field.value.clone(), Style::default().fg(theme::TEXT_DIM)));
                f.render_widget(Paragraph::new(line), input_area);
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
            Span::styled(" save  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Shift+Enter / Ctrl+J", Style::default().fg(theme::ACCENT)),
            Span::styled(" new line  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Tab", Style::default().fg(theme::ACCENT)),
            Span::styled(" next field  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Escape", Style::default().fg(theme::ACCENT)),
            Span::styled(" cancel", Style::default().fg(theme::TEXT_DIM)),
        ]);
        let footer_para = Paragraph::new(footer).alignment(Alignment::Center);
        f.render_widget(footer_para, chunks[chunk_idx]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => return ScreenAction::Pop,
                KeyCode::Enter if modifiers.is_empty() => {
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

/// Given a string with `\n` separators and a byte cursor offset, return
/// (line_index, byte_offset_within_line).
fn line_offset(s: &str, cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(s.len());
    let line_start = s[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_idx = s[..cursor].matches('\n').count();
    (line_idx, cursor - line_start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn make_screen(repos_value: &str) -> SettingsScreen {
        let fields = vec![
            Field::new("repos_dir", "Repos directories", repos_value, true),
            Field::new("worktrees_dir", "Worktrees directory", "/wt", false),
        ];
        SettingsScreen {
            fields,
            focused: 0,
            status: String::new(),
            status_is_error: false,
        }
    }

    #[test]
    fn multiline_field_renders_cursor_on_focused_line() {
        let mgr = crate::manager::Manager::new(rusqlite::Connection::open_in_memory().unwrap());
        crate::db::init_schema(&mgr.conn);
        let screen = make_screen("/abc");
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| screen.render(f, f.area(), &mgr)).unwrap();
        let buf = terminal.backend().buffer();

        // Scan the buffer for any cell with CURSOR_BG background. If we don't
        // find one, the cursor isn't rendering — that's the bug.
        let cursor_cells: Vec<_> = (0..buf.area.width)
            .flat_map(|x| (0..buf.area.height).map(move |y| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].bg == theme::CURSOR_BG)
            .collect();
        assert!(
            !cursor_cells.is_empty(),
            "expected at least one cell with CURSOR_BG; found none. Buffer dump:\n{}",
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
