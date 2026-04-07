// trellis/src/tui/review.rs — checkout a PR or branch into a worktree

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::manager::Manager;
use crate::models::Repo;
use crate::switch::SwitchAction;
use super::rename::{input_handle_key, render_input, render_modal_box};
use super::{ScreenAction, ScreenBehavior};
use super::theme;

pub struct ReviewScreen {
    repos: Vec<Repo>,
    selected_idx: usize,
    input: String,
    cursor_pos: usize,
    error: String,
    status: String,
}

impl ReviewScreen {
    #[allow(dead_code)]
    pub fn new(manager: &Manager) -> Self {
        let mut repos = manager.get_repos();

        // Sort repos by most recently active session (descending)
        let sessions = manager.get_sessions();
        let mut repo_last_active: HashMap<i64, String> = HashMap::new();
        for s in &sessions {
            if let Some(ref last) = s.last_selected_at {
                let repo_id = s.repo_id;
                if let Some(existing) = repo_last_active.get(&repo_id) {
                    if last > existing {
                        repo_last_active.insert(repo_id, last.clone());
                    }
                } else {
                    repo_last_active.insert(repo_id, last.clone());
                }
            }
        }
        repos.sort_by(|a, b| {
            let a_last = a.id.and_then(|id| repo_last_active.get(&id).cloned()).unwrap_or_default();
            let b_last = b.id.and_then(|id| repo_last_active.get(&id).cloned()).unwrap_or_default();
            b_last.cmp(&a_last)
        });

        Self {
            repos,
            selected_idx: 0,
            input: String::new(),
            cursor_pos: 0,
            error: String::new(),
            status: String::new(),
        }
    }

    fn selected_repo(&self) -> Option<&Repo> {
        self.repos.get(self.selected_idx)
    }

    fn repo_display(&self) -> Line<'_> {
        match self.selected_repo() {
            Some(repo) => Line::from(vec![
                Span::styled(&repo.name, Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD)),
                Span::styled("  ", Style::default()),
                Span::styled(&repo.path, Style::default().fg(theme::TEXT_DIM)),
            ]),
            None => Line::from(Span::styled("No repos configured", Style::default().fg(theme::RED))),
        }
    }
}

impl ScreenBehavior for ReviewScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let height: u16 = if self.repos.len() > 1 { 14 } else { 13 };
        let inner = render_modal_box(f, area, "Review", 70, height);

        let mut constraints = vec![
            Constraint::Length(1), // spacer
        ];
        if self.repos.len() > 1 {
            constraints.push(Constraint::Length(1)); // repo label with tab hint
        }
        constraints.extend([
            Constraint::Length(1), // repo display
            Constraint::Length(1), // spacer
            Constraint::Length(1), // hint label
            Constraint::Length(1), // input
            Constraint::Length(1), // spacer
            Constraint::Length(1), // error/status
            Constraint::Length(1), // spacer
            Constraint::Length(1), // footer hint
        ]);

        let chunks = Layout::vertical(constraints).split(inner);
        let mut row = 1; // skip spacer at index 0

        // Repo label with tab hint (only if multiple repos)
        if self.repos.len() > 1 {
            let label = Line::from(vec![
                Span::styled("Repo ", Style::default().fg(theme::TEXT_DIM)),
                Span::styled("(tab to cycle)", Style::default().fg(theme::TEXT_DIM)),
            ]);
            f.render_widget(Paragraph::new(label), chunks[row]);
            row += 1;
        }

        // Repo name + path
        f.render_widget(Paragraph::new(self.repo_display()), chunks[row]);
        row += 1;

        // Spacer
        row += 1;

        // "PR number or branch name" hint
        let hint = Paragraph::new(Span::styled(
            "PR number or branch name",
            Style::default().fg(theme::TEXT_DIM),
        ));
        f.render_widget(hint, chunks[row]);
        row += 1;

        // Text input
        render_input(f, chunks[row], &self.input, self.cursor_pos);
        row += 1;

        // Spacer
        row += 1;

        // Error or status line
        if !self.status.is_empty() {
            let status = Paragraph::new(Span::styled(&self.status, Style::default().fg(theme::TEXT_DIM)));
            f.render_widget(status, chunks[row]);
        } else if !self.error.is_empty() {
            let err = Paragraph::new(Span::styled(&self.error, theme::style_error()));
            f.render_widget(err, chunks[row]);
        }
        row += 1;

        // Spacer
        row += 1;

        // Footer hint
        let footer = Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme::ACCENT)),
            Span::styled(" to checkout  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Escape", Style::default().fg(theme::ACCENT)),
            Span::styled(" to cancel", Style::default().fg(theme::TEXT_DIM)),
        ]);
        let footer_para = Paragraph::new(footer).alignment(Alignment::Center);
        f.render_widget(footer_para, chunks[row]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => return ScreenAction::Pop,
                KeyCode::Tab => {
                    if self.repos.len() > 1 {
                        self.selected_idx = (self.selected_idx + 1) % self.repos.len();
                        self.error.clear();
                    }
                }
                KeyCode::Enter => {
                    if self.selected_repo().is_none() {
                        self.error = "No repos configured. Create a session first.".to_string();
                        return ScreenAction::None;
                    }

                    let value = self.input.trim().to_string();
                    if value.is_empty() {
                        self.error = "Please enter a PR number or branch name.".to_string();
                        return ScreenAction::None;
                    }

                    self.status = "Checking out...".to_string();
                    self.error.clear();

                    let repo_path = self.selected_repo().unwrap().path.clone();
                    match manager.checkout_and_review(&repo_path, &value) {
                        Ok((session, _worktree_path)) => {
                            return ScreenAction::Switch(SwitchAction::Session {
                                target: session.name,
                            });
                        }
                        Err(e) => {
                            self.status.clear();
                            self.error = e;
                            return ScreenAction::None;
                        }
                    }
                }
                _ => {
                    input_handle_key(&mut self.input, &mut self.cursor_pos, *code, *modifiers);
                    self.error.clear();
                    self.status.clear();
                }
            }
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        true
    }
}
