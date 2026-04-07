// torchard-rs/src/tui/edit_branch.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::git;
use crate::manager::Manager;
use super::{ScreenAction, ScreenBehavior};
use super::theme;

pub struct EditBranchScreen {
    session_id: i64,
    session_name: String,
    // All branches loaded on mount
    all_branches: Vec<String>,
    // Currently displayed (filtered) branches + optional "new branch" entry
    display: Vec<DisplayItem>,
    list_state: ListState,
    filter: String,
    filter_cursor: usize,
    error: String,
}

enum DisplayItem {
    Branch(String),
    NewBranch(String),
}

impl DisplayItem {
    fn label(&self) -> String {
        match self {
            DisplayItem::Branch(b) => b.clone(),
            DisplayItem::NewBranch(b) => format!("+ Use: {}", b),
        }
    }
}

impl EditBranchScreen {
    pub fn new(manager: &Manager, session_id: i64, session_name: String) -> Self {
        let mut screen = Self {
            session_id,
            session_name,
            all_branches: Vec::new(),
            display: Vec::new(),
            list_state: ListState::default(),
            filter: String::new(),
            filter_cursor: 0,
            error: String::new(),
        };
        screen.load_branches(manager);
        screen
    }

    fn load_branches(&mut self, manager: &Manager) {
        // Find the session's repo
        let sessions = manager.get_sessions();
        let session = match sessions.iter().find(|s| s.id == Some(self.session_id)) {
            Some(s) => s.clone(),
            None => {
                self.error = "Session not found".to_string();
                return;
            }
        };

        let repos = manager.get_repos();
        let repo = match repos.iter().find(|r| r.id == Some(session.repo_id)) {
            Some(r) => r.clone(),
            None => {
                self.error = "Repo not found".to_string();
                return;
            }
        };

        match git::list_branches(&repo.path) {
            Ok(branches) => {
                self.all_branches = branches;
            }
            Err(e) => {
                self.error = e.to_string();
            }
        }

        self.rebuild_display();
    }

    fn rebuild_display(&mut self) {
        let query = self.filter.to_lowercase();
        let filtered: Vec<String> = if query.is_empty() {
            self.all_branches.clone()
        } else {
            self.all_branches
                .iter()
                .filter(|b| b.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };

        self.display = filtered
            .into_iter()
            .map(DisplayItem::Branch)
            .collect();

        // Add "New branch" option if filter text is non-empty and not an exact match
        let typed = self.filter.trim().to_string();
        if !typed.is_empty() {
            let is_exact = self.all_branches.iter().any(|b| b == &typed);
            if !is_exact {
                self.display.push(DisplayItem::NewBranch(typed));
            }
        }

        // Reset selection
        if self.display.is_empty() {
            self.list_state.select(None);
        } else {
            let sel = self.list_state.selected().unwrap_or(0).min(self.display.len() - 1);
            self.list_state.select(Some(sel));
        }
    }

    fn apply_selected(&mut self, manager: &mut Manager) -> ScreenAction {
        let idx = match self.list_state.selected() {
            Some(i) => i,
            None => return ScreenAction::None,
        };
        let branch = match self.display.get(idx) {
            Some(DisplayItem::Branch(b)) => b.clone(),
            Some(DisplayItem::NewBranch(b)) => b.clone(),
            None => return ScreenAction::None,
        };
        self.apply_branch(&branch, manager)
    }

    fn apply_branch(&mut self, branch: &str, manager: &mut Manager) -> ScreenAction {
        match manager.set_base_branch(self.session_id, branch) {
            Ok(_) => ScreenAction::Pop,
            Err(e) => {
                self.error = e;
                ScreenAction::None
            }
        }
    }

    fn handle_filter_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.insert(self.filter_cursor, c);
                self.filter_cursor += c.len_utf8();
                self.rebuild_display();
            }
            KeyCode::Backspace => {
                if self.filter_cursor > 0 {
                    let prev = self.filter[..self.filter_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.filter.drain(prev..self.filter_cursor);
                    self.filter_cursor = prev;
                    self.rebuild_display();
                }
            }
            KeyCode::Left => {
                if self.filter_cursor > 0 {
                    let prev = self.filter[..self.filter_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.filter_cursor = prev;
                }
            }
            KeyCode::Right => {
                if self.filter_cursor < self.filter.len() {
                    let next = self.filter[self.filter_cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.filter_cursor + i)
                        .unwrap_or(self.filter.len());
                    self.filter_cursor = next;
                }
            }
            KeyCode::Home => {
                self.filter_cursor = 0;
            }
            KeyCode::End => {
                self.filter_cursor = self.filter.len();
            }
            _ => {}
        }
    }
}

impl ScreenBehavior for EditBranchScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let width = 92u16;
        let list_height = 18u16;
        // title + hint + filter + list + error + hint = ~24 + borders
        let height = (list_height + 8).min(area.height.saturating_sub(4));

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
                format!(" Edit Branch — {} ", self.session_name),
                Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT))
            .style(Style::default().fg(theme::TEXT).bg(theme::HEADER_BG));

        let inner = block.inner(box_area);
        f.render_widget(block, box_area);

        let chunks = Layout::vertical([
            Constraint::Length(1), // hint
            Constraint::Length(1), // gap
            Constraint::Length(1), // filter label + input
            Constraint::Length(list_height), // list
            Constraint::Length(1), // error
        ])
        .split(inner);

        // Hint
        let hint = Paragraph::new(Span::styled(
            "Pick the branch new worktrees will branch from.",
            Style::default().fg(theme::TEXT_DIM),
        ))
        .alignment(Alignment::Center);
        f.render_widget(hint, chunks[0]);

        // Filter input
        let before = &self.filter[..self.filter_cursor];
        let cursor_char_len = self.filter[self.filter_cursor..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        let cursor_display = if self.filter_cursor < self.filter.len() {
            self.filter[self.filter_cursor..self.filter_cursor + cursor_char_len].to_string()
        } else {
            " ".to_string()
        };
        let after = if self.filter_cursor + cursor_char_len <= self.filter.len() {
            &self.filter[self.filter_cursor + cursor_char_len..]
        } else {
            ""
        };

        let filter_line = Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled(before.to_string(), Style::default().fg(theme::TEXT)),
            Span::styled(cursor_display, theme::style_cursor()),
            Span::styled(after.to_string(), Style::default().fg(theme::TEXT)),
        ]);
        f.render_widget(Paragraph::new(filter_line), chunks[2]);

        // Branch list
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::TEXT_DIM));
        let list_inner = list_block.inner(chunks[3]);
        f.render_widget(list_block, chunks[3]);

        let items: Vec<ListItem> = self
            .display
            .iter()
            .map(|item| {
                let style = match item {
                    DisplayItem::Branch(_) => Style::default().fg(theme::TEXT),
                    DisplayItem::NewBranch(_) => Style::default().fg(theme::GREEN),
                };
                ListItem::new(Span::styled(item.label(), style))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(theme::style_cursor())
            .highlight_symbol("> ");

        f.render_stateful_widget(list, list_inner, &mut self.list_state.clone());

        // Error
        if !self.error.is_empty() {
            let err = Paragraph::new(Span::styled(self.error.clone(), theme::style_error()))
                .alignment(Alignment::Center);
            f.render_widget(err, chunks[4]);
        }
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => return ScreenAction::Pop,
                KeyCode::Enter => {
                    // If nothing selected but filter has text, apply filter text directly
                    if self.list_state.selected().is_none() {
                        let typed = self.filter.trim().to_string();
                        if !typed.is_empty() {
                            return self.apply_branch(&typed, manager);
                        }
                    } else {
                        return self.apply_selected(manager);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let next = match self.list_state.selected() {
                        Some(i) if i + 1 < self.display.len() => i + 1,
                        Some(i) => i,
                        None if !self.display.is_empty() => 0,
                        None => return ScreenAction::None,
                    };
                    self.list_state.select(Some(next));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let prev = match self.list_state.selected() {
                        Some(0) | None => 0,
                        Some(i) => i - 1,
                    };
                    self.list_state.select(Some(prev));
                }
                _ => {
                    self.handle_filter_key(*code, *modifiers);
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
