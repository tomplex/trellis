// trellis/src/tui/cleanup.rs — worktree cleanup screen

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::git;
use crate::manager::Manager;
use crate::models::Worktree;
use crate::utils::truncate_start;

use super::confirm::ConfirmScreen;
use super::theme;
use super::{ActionResult, ScreenAction, ScreenBehavior};

struct WorktreeRow {
    wt_id: i64,
    branch: String,
    session_name: String,
    path: String,
    stale: Option<bool>, // None = still checking
}

pub struct CleanupScreen {
    rows: Vec<WorktreeRow>,
    selected: HashSet<i64>,
    table_state: TableState,
    error_message: Option<String>,
    pending_delete: bool,
    stale_rx: Option<mpsc::Receiver<HashSet<i64>>>,
}

impl CleanupScreen {
    pub fn new(manager: &Manager) -> Self {
        let worktrees = manager.get_all_worktrees();
        let sessions = manager.get_sessions();

        // Build session name lookup: session_id -> session name
        let session_map: HashMap<i64, String> = sessions
            .into_iter()
            .filter_map(|s| s.id.map(|id| (id, s.name.clone())))
            .collect();

        // Build rows immediately with stale=None (checking)
        let mut rows: Vec<WorktreeRow> = worktrees
            .iter()
            .filter_map(|wt| {
                let wt_id = wt.id?;
                let session_name = wt
                    .session_id
                    .and_then(|sid| session_map.get(&sid).cloned())
                    .unwrap_or_else(|| "Unattached".to_string());
                Some(WorktreeRow {
                    wt_id,
                    branch: wt.branch.clone(),
                    session_name,
                    path: wt.path.clone(),
                    stale: None, // checking in background
                })
            })
            .collect();

        // Sort by session name then branch
        rows.sort_by(|a, b| {
            a.session_name
                .to_lowercase()
                .cmp(&b.session_name.to_lowercase())
                .then_with(|| a.branch.to_lowercase().cmp(&b.branch.to_lowercase()))
        });

        let mut table_state = TableState::default();
        if !rows.is_empty() {
            table_state.select(Some(0));
        }

        // Spawn background thread for staleness check
        let (tx, rx) = mpsc::channel();
        let owned_worktrees: Vec<Worktree> = worktrees;
        let repos = manager.get_repos();
        std::thread::spawn(move || {
            let repo_map: HashMap<i64, crate::models::Repo> = repos
                .into_iter()
                .filter_map(|r| r.id.map(|id| (id, r)))
                .collect();
            let mut stale_ids = HashSet::new();
            for wt in &owned_worktrees {
                let Some(wt_id) = wt.id else { continue };
                let Some(repo) = repo_map.get(&wt.repo_id) else { continue };
                let merged = git::is_branch_merged(&repo.path, &wt.branch, &repo.default_branch)
                    .unwrap_or(false);
                let has_remote = git::has_remote_branch(&repo.path, &wt.branch)
                    .unwrap_or(true);
                if merged || !has_remote {
                    stale_ids.insert(wt_id);
                }
            }
            let _ = tx.send(stale_ids);
        });

        Self {
            rows,
            selected: HashSet::new(),
            table_state,
            error_message: None,
            pending_delete: false,
            stale_rx: Some(rx),
        }
    }

    fn check_stale_results(&mut self) {
        if let Some(ref rx) = self.stale_rx {
            if let Ok(stale_ids) = rx.try_recv() {
                for row in &mut self.rows {
                    row.stale = Some(stale_ids.contains(&row.wt_id));
                }
                self.stale_rx = None;
            }
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let max = self.rows.len() as i32 - 1;
        let next = (current + delta).clamp(0, max) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle_current(&mut self) {
        if let Some(idx) = self.table_state.selected() {
            if idx < self.rows.len() {
                let wt_id = self.rows[idx].wt_id;
                if !self.selected.remove(&wt_id) {
                    self.selected.insert(wt_id);
                }
            }
        }
    }

    fn select_all(&mut self) {
        for row in &self.rows {
            self.selected.insert(row.wt_id);
        }
    }

    fn deselect_all(&mut self) {
        self.selected.clear();
    }

    fn stale_count(&self) -> usize {
        self.rows.iter().filter(|r| r.stale == Some(true)).count()
    }

    fn status_line(&self) -> Line<'_> {
        let total = self.rows.len();
        let stale = self.stale_count();
        let sel = self.selected.len();

        Line::from(vec![
            Span::styled(
                format!(" {} worktrees", total),
                Style::default().fg(theme::TEXT_DIM),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} stale", stale),
                Style::default().fg(theme::YELLOW),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} selected", sel),
                Style::default().fg(theme::ACCENT),
            ),
        ])
    }

    fn perform_delete(&mut self, manager: &mut Manager) {
        let ids_to_delete: Vec<i64> = self.selected.iter().copied().collect();
        let mut errors = Vec::new();

        for wt_id in &ids_to_delete {
            if let Err(e) = manager.cleanup_worktree(*wt_id) {
                errors.push(format!("worktree {}: {}", wt_id, e));
            } else {
                self.rows.retain(|r| r.wt_id != *wt_id);
            }
        }

        // Remove successfully deleted from selection
        for row_id in &ids_to_delete {
            if !errors.iter().any(|e| e.starts_with(&format!("worktree {}:", row_id))) {
                self.selected.remove(row_id);
            }
        }

        if !errors.is_empty() {
            self.error_message = Some(errors.join("; "));
        } else {
            self.error_message = None;
        }

        // Fix cursor if it's past the end
        if !self.rows.is_empty() {
            let max = self.rows.len() - 1;
            let current = self.table_state.selected().unwrap_or(0);
            if current > max {
                self.table_state.select(Some(max));
            }
        } else {
            self.table_state.select(None);
        }

        self.pending_delete = false;
    }

    fn render_footer(&self) -> Paragraph<'_> {
        super::rename::render_footer_bindings(&[
            ("esc", "back"),
            ("j/k", "navigate"),
            ("space", "toggle"),
            ("a", "select all"),
            ("A", "deselect all"),
            ("d", "delete"),
        ])
    }
}

impl ScreenBehavior for CleanupScreen {
    fn tick(&mut self, _manager: &mut Manager) -> ScreenAction {
        self.check_stale_results();
        ScreenAction::None
    }

    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let error_height = if self.error_message.is_some() { 1 } else { 0 };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // status bar
                Constraint::Length(error_height), // error
                Constraint::Min(0),    // table
                Constraint::Length(1), // footer
            ])
            .split(area);

        // Title
        let title = Paragraph::new(Line::from(vec![Span::styled(
            " Cleanup Worktrees",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(title, chunks[0]);

        // Status bar
        let status = Paragraph::new(self.status_line()).style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(status, chunks[1]);

        // Error
        if let Some(ref err) = self.error_message {
            let err_widget =
                Paragraph::new(format!(" Error: {}", err)).style(Style::default().fg(theme::RED));
            f.render_widget(err_widget, chunks[2]);
        }

        // Table
        let header = Row::new(vec![
            Cell::from(" "),
            Cell::from("Branch"),
            Cell::from("Session"),
            Cell::from("Path"),
            Cell::from("Status"),
        ])
        .style(theme::style_header())
        .height(1);

        let table_rows: Vec<Row> = self
            .rows
            .iter()
            .map(|row| {
                let is_selected = self.selected.contains(&row.wt_id);

                let checkbox = if is_selected {
                    Span::styled("[x]", Style::default().fg(theme::BLUE))
                } else {
                    Span::styled("[ ]", Style::default().fg(theme::TEXT_DIM))
                };

                let branch_style = if row.stale == Some(true) {
                    Style::default().fg(theme::YELLOW)
                } else {
                    Style::default().fg(theme::TEXT)
                };

                let status_span = match row.stale {
                    None => Span::styled("checking…", Style::default().fg(theme::TEXT_DIM)),
                    Some(true) => Span::styled("stale", Style::default().fg(theme::YELLOW)),
                    Some(false) => Span::styled("ok", Style::default().fg(theme::GREEN)),
                };

                Row::new(vec![
                    Cell::from(checkbox),
                    Cell::from(Span::styled(&*row.branch, branch_style)),
                    Cell::from(Span::styled(&*row.session_name, Style::default().fg(theme::TEXT))),
                    Cell::from(Span::styled(
                        truncate_start(&row.path, 40),
                        Style::default().fg(theme::TEXT_DIM),
                    )),
                    Cell::from(status_span),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(3),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Length(10),
        ];

        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(theme::style_cursor())
            .style(theme::style_default())
            .block(Block::default().style(theme::style_default()));

        let mut state = self.table_state.clone();
        f.render_stateful_widget(table, chunks[3], &mut state);

        // Footer
        let footer = self.render_footer();
        f.render_widget(footer, chunks[4]);
    }

    fn handle_event(&mut self, event: &Event, _manager: &mut Manager) -> ScreenAction {
        // Always check for background staleness results (even on non-key events)
        self.check_stale_results();

        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return ScreenAction::None;
        };

        match code {
            KeyCode::Esc => ScreenAction::Pop,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor(1);
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor(-1);
                ScreenAction::None
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.toggle_current();
                ScreenAction::None
            }
            KeyCode::Char('A') => {
                // Shift+A: deselect all
                self.deselect_all();
                ScreenAction::None
            }
            KeyCode::Char('a') => {
                self.select_all();
                ScreenAction::None
            }
            KeyCode::Char('d') => {
                if self.selected.is_empty() {
                    return ScreenAction::None;
                }
                let count = self.selected.len();
                self.pending_delete = true;
                ScreenAction::Push(Box::new(ConfirmScreen::new(
                    "Confirm Deletion".to_string(),
                    format!("Delete {} worktree(s)?", count),
                )))
            }
            _ => ScreenAction::None,
        }
    }

    fn on_child_result(&mut self, result: ActionResult, manager: &mut Manager) -> ScreenAction {
        if let ActionResult::Confirmed(true) = result {
            if self.pending_delete {
                self.perform_delete(manager);
            }
        } else {
            self.pending_delete = false;
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        false
    }
}
