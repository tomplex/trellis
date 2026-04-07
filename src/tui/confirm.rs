// torchard-rs/src/tui/confirm.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::manager::Manager;
use super::{ActionResult, ScreenAction, ScreenBehavior};
use super::theme;

pub struct ConfirmScreen {
    title: String,
    body: String,
}

impl ConfirmScreen {
    pub fn new(title: String, body: String) -> Self {
        Self { title, body }
    }
}

impl ScreenBehavior for ConfirmScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        // Centered box: 70 wide, ~8 tall
        let height = 8u16;
        let width = 72u16;

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

        let container_style = Style::default()
            .fg(theme::TEXT)
            .bg(theme::HEADER_BG);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT))
            .style(container_style);

        let inner = block.inner(box_area);
        f.render_widget(block, box_area);

        // Split inner: title, gap, body, gap, buttons
        let chunks = Layout::vertical([
            Constraint::Length(1), // title
            Constraint::Length(1), // gap
            Constraint::Length(1), // body
            Constraint::Length(1), // gap
            Constraint::Length(1), // buttons
        ])
        .split(inner);

        // Title (red, bold, centered)
        let title_para = Paragraph::new(self.title.clone())
            .style(Style::default().fg(theme::RED).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        f.render_widget(title_para, chunks[0]);

        // Body (white, centered)
        let body_para = Paragraph::new(self.body.clone())
            .style(Style::default().fg(theme::TEXT))
            .alignment(Alignment::Center);
        f.render_widget(body_para, chunks[2]);

        // Buttons
        let buttons = Line::from(vec![
            Span::styled("[y]", Style::default().fg(theme::GREEN).add_modifier(Modifier::BOLD)),
            Span::styled(" Yes    ", Style::default().fg(theme::TEXT)),
            Span::styled("[n]", Style::default().fg(theme::RED).add_modifier(Modifier::BOLD)),
            Span::styled(" No", Style::default().fg(theme::TEXT)),
        ]);
        let buttons_para = Paragraph::new(buttons).alignment(Alignment::Center);
        f.render_widget(buttons_para, chunks[4]);
    }

    fn handle_event(&mut self, event: &Event, _manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) = event {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    return ScreenAction::PopWith(ActionResult::Confirmed(true));
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    return ScreenAction::PopWith(ActionResult::Confirmed(false));
                }
                _ => {}
            }
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        true
    }
}
