// trellis/src/tui/action_menu.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::manager::Manager;
use super::{ActionResult, ScreenAction, ScreenBehavior};
use super::theme;

pub struct ActionMenuScreen {
    title: String,
    items: Vec<(String, String, String)>, // (key, label, hint)
    state: ListState,
}

impl ActionMenuScreen {
    #[allow(dead_code)]
    pub fn new(title: String, items: Vec<(String, String, String)>) -> Self {
        let mut state = ListState::default();
        if !items.is_empty() {
            state.select(Some(0));
        }
        Self { title, items, state }
    }
}

impl ScreenBehavior for ActionMenuScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        // Center the modal: 50 wide, auto height capped at 20
        let height = (self.items.len() as u16 + 4).min(20);
        let width = 52u16;

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
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT))
            .style(container_style);

        let inner = block.inner(box_area);
        f.render_widget(block, box_area);

        let list_items: Vec<ListItem> = self
            .items
            .iter()
            .map(|(_key, label, hint)| {
                if hint.is_empty() {
                    ListItem::new(Line::from(vec![
                        Span::styled(label.clone(), Style::default().fg(theme::TEXT)),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        Span::styled(label.clone(), Style::default().fg(theme::TEXT)),
                        Span::styled(
                            format!("  {}", hint),
                            Style::default().fg(theme::TEXT_DIM),
                        ),
                    ]))
                }
            })
            .collect();

        let list = List::new(list_items)
            .highlight_style(theme::style_cursor())
            .highlight_symbol("> ");

        f.render_stateful_widget(list, inner, &mut self.state.clone());
    }

    fn handle_event(&mut self, event: &Event, _manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) = event {
            match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    let next = match self.state.selected() {
                        Some(i) if i + 1 < self.items.len() => i + 1,
                        Some(i) => i,
                        None => 0,
                    };
                    self.state.select(Some(next));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    let prev = match self.state.selected() {
                        Some(0) | None => 0,
                        Some(i) => i - 1,
                    };
                    self.state.select(Some(prev));
                }
                KeyCode::Enter => {
                    if let Some(i) = self.state.selected() {
                        if let Some((key, _, _)) = self.items.get(i) {
                            return ScreenAction::PopWith(ActionResult::MenuPick(Some(key.clone())));
                        }
                    }
                }
                KeyCode::Esc => {
                    return ScreenAction::PopWith(ActionResult::MenuPick(None));
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
