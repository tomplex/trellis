// trellis/src/tui/new_tab.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::manager::Manager;
use crate::switch::{write_switch, SwitchAction};
use crate::tmux;
use super::rename::{input_handle_key, render_input, render_modal_box};
use super::{ScreenAction, ScreenBehavior};
use super::theme;

pub struct NewTabScreen {
    session_id: i64,
    session_name: String,
    input: String,
    cursor_pos: usize,
    error: String,
}

impl NewTabScreen {
    #[allow(dead_code)]
    pub fn new(_manager: &Manager, session_id: i64, session_name: String) -> Self {
        Self {
            session_id,
            session_name,
            input: String::new(),
            cursor_pos: 0,
            error: String::new(),
        }
    }
}

impl ScreenBehavior for NewTabScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let title = format!("New Tab — session {}", self.session_name);
        let inner = render_modal_box(f, area, &title, 74, 9);

        let chunks = Layout::vertical([
            Constraint::Length(1), // label
            Constraint::Length(1), // input
            Constraint::Length(1), // error
            Constraint::Length(1), // hint
        ])
        .split(inner);

        // Label
        let label = Paragraph::new(Span::styled("Branch name:", Style::default().fg(theme::TEXT_DIM)));
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
            Span::styled(" to create  ", Style::default().fg(theme::TEXT_DIM)),
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
                    let branch_name = self.input.trim().to_string();
                    if branch_name.is_empty() {
                        self.error = "Branch name cannot be empty.".to_string();
                        return ScreenAction::None;
                    }
                    match manager.add_tab(self.session_id, &branch_name) {
                        Err(e) => {
                            self.error = e;
                            return ScreenAction::None;
                        }
                        Ok(_) => {}
                    }
                    let target = format!("{}:{}", self.session_name, branch_name);
                    tmux::send_keys(&target, &["claude", "Enter"]);
                    write_switch(&SwitchAction::Session {
                        target: self.session_name.clone(),
                    });
                    return ScreenAction::Quit;
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
