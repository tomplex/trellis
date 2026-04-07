// torchard-rs/src/tui/help.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::manager::Manager;
use super::{ScreenAction, ScreenBehavior};
use super::theme;

pub struct HelpScreen;

fn help_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("torchard", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" — tmux session & worktree manager", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(""),
        Line::from(Span::styled("Session List", Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("enter", Style::default().fg(theme::ACCENT)),
            Span::styled("     Switch to session/tab", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("tab", Style::default().fg(theme::ACCENT)),
            Span::styled("       Expand/collapse tabs", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("n", Style::default().fg(theme::ACCENT)),
            Span::styled("         New\u{2026} (session, tab, or PR review)", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("d", Style::default().fg(theme::ACCENT)),
            Span::styled("         Delete session or kill tab", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("h", Style::default().fg(theme::ACCENT)),
            Span::styled("         Conversation history", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(".", Style::default().fg(theme::ACCENT)),
            Span::styled("         Actions menu", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("j/k", Style::default().fg(theme::ACCENT)),
            Span::styled("       Navigate up/down", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("/", Style::default().fg(theme::ACCENT)),
            Span::styled("         Filter sessions", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("q", Style::default().fg(theme::ACCENT)),
            Span::styled("         Quit", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(""),
        Line::from(Span::styled("Actions menu (.)", Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  Rename, change branch, launch claude,", Style::default().fg(theme::TEXT))),
        Line::from(Span::styled("  adopt, cleanup, settings", Style::default().fg(theme::TEXT))),
        Line::from(""),
        Line::from(Span::styled("Cleanup View", Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("space/enter", Style::default().fg(theme::ACCENT)),
            Span::styled("  Toggle selection", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("a", Style::default().fg(theme::ACCENT)),
            Span::styled("           Select all", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("d", Style::default().fg(theme::ACCENT)),
            Span::styled("           Delete selected", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("escape", Style::default().fg(theme::ACCENT)),
            Span::styled("      Back", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(""),
        Line::from(Span::styled("Press Escape to close this help.", Style::default().fg(theme::TEXT_DIM))),
    ]
}

impl ScreenBehavior for HelpScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let lines = help_lines();
        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
        let width = 54u16;

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

        let para = Paragraph::new(lines).style(Style::default().fg(theme::TEXT));
        f.render_widget(para, inner);
    }

    fn handle_event(&mut self, event: &Event, _manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) = event {
            match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    return ScreenAction::Pop;
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
