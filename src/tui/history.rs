// torchard-rs/src/tui/history.rs — conversation history browser

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::conversation_index;
use crate::manager::Manager;
use crate::models::Conversation;
use crate::switch::{self, SwitchAction};
use crate::tmux;
use crate::utils::truncate_end;

use super::theme;
use super::{ScreenAction, ScreenBehavior};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Date,
    Project,
    Branch,
    Summary,
}

impl SortMode {
    fn label(&self) -> &'static str {
        match self {
            SortMode::Date => "date",
            SortMode::Project => "project",
            SortMode::Branch => "branch",
            SortMode::Summary => "summary",
        }
    }

    fn default_descending(&self) -> bool {
        matches!(self, SortMode::Date)
    }
}

pub struct HistoryScreen {
    scope_paths: Option<Vec<String>>,
    scope_label: Option<String>,
    scoped: bool,

    all_entries: Vec<Conversation>,
    scoped_entries: Vec<Conversation>,
    displayed: Vec<Conversation>,

    filter: String,
    filter_active: bool,

    sort_by: SortMode,
    sort_descending: bool,

    table_state: TableState,
}

impl HistoryScreen {
    pub fn new(
        _manager: &Manager,
        scope_paths: Option<Vec<String>>,
        scope_label: Option<String>,
    ) -> Self {
        let all_entries = conversation_index::parse_index(None);
        let scoped = scope_paths.is_some();
        let scoped_entries = match &scope_paths {
            Some(paths) => conversation_index::filter_by_paths(&all_entries, paths),
            None => Vec::new(),
        };

        let mut screen = Self {
            scope_paths,
            scope_label,
            scoped,
            all_entries,
            scoped_entries,
            displayed: Vec::new(),
            filter: String::new(),
            filter_active: false,
            sort_by: SortMode::Date,
            sort_descending: true,
            table_state: TableState::default(),
        };
        screen.rebuild();
        screen
    }

    fn rebuild(&mut self) {
        let entries = if self.scoped && self.scope_paths.is_some() {
            &self.scoped_entries
        } else {
            &self.all_entries
        };

        let filtered: Vec<Conversation> = if self.filter.is_empty() {
            entries.to_vec()
        } else {
            let q = &self.filter;
            entries
                .iter()
                .filter(|e| {
                    e.date.to_lowercase().contains(q)
                        || e.project.to_lowercase().contains(q)
                        || e.branch.to_lowercase().contains(q)
                        || e.summary().to_lowercase().contains(q)
                })
                .cloned()
                .collect()
        };

        let mut sorted = filtered;
        let desc = self.sort_descending;
        match self.sort_by {
            SortMode::Date => sorted.sort_by(|a, b| {
                let cmp = a.date.cmp(&b.date);
                if desc { cmp.reverse() } else { cmp }
            }),
            SortMode::Project => sorted.sort_by(|a, b| {
                let cmp = a.project.to_lowercase().cmp(&b.project.to_lowercase());
                if desc { cmp.reverse() } else { cmp }
            }),
            SortMode::Branch => sorted.sort_by(|a, b| {
                let cmp = a.branch.to_lowercase().cmp(&b.branch.to_lowercase());
                if desc { cmp.reverse() } else { cmp }
            }),
            SortMode::Summary => sorted.sort_by(|a, b| {
                let cmp = a.summary().to_lowercase().cmp(&b.summary().to_lowercase());
                if desc { cmp.reverse() } else { cmp }
            }),
        }

        self.displayed = sorted;

        // Reset cursor to top if there are entries
        if !self.displayed.is_empty() {
            self.table_state.select(Some(0));
        } else {
            self.table_state.select(None);
        }
    }

    fn title_line(&self) -> Line<'_> {
        let count = self.displayed.len();
        let arrow = if self.sort_descending { "\u{2193}" } else { "\u{2191}" };

        let scope_text = if self.scoped {
            if let Some(ref label) = self.scope_label {
                format!("scoped to {}", label)
            } else {
                "scoped".to_string()
            }
        } else {
            "all conversations".to_string()
        };

        Line::from(vec![
            Span::styled(" History", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("  ", Style::default()),
            Span::styled(scope_text, Style::default().fg(theme::TEXT_DIM)),
            Span::styled(format!("  ({count})  "), Style::default().fg(theme::TEXT_DIM)),
            Span::styled(format!("sort: {} {}", self.sort_by.label(), arrow), Style::default().fg(theme::TEXT_DIM)),
        ])
    }

    fn shorten_project(project: &str) -> String {
        if let Some(home) = dirs::home_dir() {
            let home_str = format!("{}/", home.display());
            if project.starts_with(&home_str) {
                return format!("~/{}", &project[home_str.len()..]);
            }
        }
        project.to_string()
    }

    fn sort(&mut self, mode: SortMode) {
        if self.sort_by == mode {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_by = mode;
            self.sort_descending = mode.default_descending();
        }
        self.rebuild();
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.displayed.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let max = self.displayed.len() as i32 - 1;
        let next = (current + delta).clamp(0, max) as usize;
        self.table_state.select(Some(next));
    }

    fn activate_filter(&mut self) {
        self.filter_active = true;
        self.filter.clear();
    }

    fn dismiss_filter(&mut self) {
        self.filter_active = false;
        if !self.filter.is_empty() {
            self.filter.clear();
            self.rebuild();
        }
    }

    fn handle_filter_key(&mut self, code: KeyCode) -> ScreenAction {
        match code {
            KeyCode::Esc => {
                self.dismiss_filter();
                ScreenAction::None
            }
            KeyCode::Enter => {
                // Keep filter text, dismiss input mode
                self.filter_active = false;
                ScreenAction::None
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.rebuild();
                ScreenAction::None
            }
            KeyCode::Char(c) => {
                self.filter.push(c.to_ascii_lowercase());
                self.rebuild();
                ScreenAction::None
            }
            _ => ScreenAction::None,
        }
    }

    fn action_resume(&mut self, manager: &Manager) -> ScreenAction {
        let idx = match self.table_state.selected() {
            Some(i) if i < self.displayed.len() => i,
            _ => return ScreenAction::None,
        };
        let entry = &self.displayed[idx];

        // Resolve full session ID
        let session_id = conversation_index::resolve_session_id(&entry.session_id, &entry.project);
        let resume_cmd = format!("claude --resume {}", session_id);

        // Look for a matching managed session
        let sessions = manager.get_sessions();
        let repos = manager.get_repos();
        let repos_by_id: std::collections::HashMap<i64, _> = repos
            .into_iter()
            .filter_map(|r| r.id.map(|id| (id, r)))
            .collect();

        let mut target_session: Option<String> = None;

        for s in &sessions {
            // Check repo path
            if let Some(repo) = repos_by_id.get(&s.repo_id) {
                if entry.project.starts_with(&repo.path) {
                    target_session = Some(s.name.clone());
                    break;
                }
            }
            // Check worktree paths
            if let Some(sid) = s.id {
                for wt in manager.get_worktrees_for_session(sid) {
                    if entry.project.starts_with(&wt.path) {
                        target_session = Some(s.name.clone());
                        break;
                    }
                }
            }
            if target_session.is_some() {
                break;
            }
        }

        if let Some(ref sess_name) = target_session {
            // Open in existing session
            let window_name = format!("resume-{}", &session_id[..session_id.len().min(8)]);
            let _ = tmux::new_window(sess_name, &window_name, Some(&entry.project));
            let target = format!("{}:{}", sess_name, window_name);
            tmux::send_keys(&target, &[&resume_cmd, "Enter"]);
            switch::write_switch(&SwitchAction::Session {
                target: sess_name.clone(),
            });
        } else {
            // Create new tmux session
            let session_name = format!("resume-{}", &session_id[..session_id.len().min(8)]);
            let _ = tmux::new_session(&session_name, &entry.project);
            tmux::send_keys(&session_name, &[&resume_cmd, "Enter"]);
            switch::write_switch(&SwitchAction::Session {
                target: session_name,
            });
        }

        ScreenAction::Quit
    }

    fn render_footer(&self) -> Paragraph<'_> {
        let bindings: Vec<(&str, &str)> = if self.filter_active {
            vec![("esc", "dismiss"), ("enter", "keep")]
        } else {
            let mut b = vec![
                ("esc", "back"),
                ("j/k", "navigate"),
                ("enter", "resume"),
                ("/", "filter"),
                ("d/p/b/s", "sort"),
            ];
            if self.scope_paths.is_some() {
                b.push(("t", "scope"));
            }
            b
        };

        let spans: Vec<Span> = bindings
            .iter()
            .enumerate()
            .flat_map(|(i, (key, desc))| {
                let mut v = Vec::new();
                if i > 0 {
                    v.push(Span::styled("  ", theme::style_footer()));
                }
                v.push(Span::styled(*key, theme::style_footer_key()));
                v.push(Span::styled(format!(" {}", desc), theme::style_footer()));
                v
            })
            .collect();

        Paragraph::new(Line::from(spans)).style(theme::style_footer())
    }
}

impl ScreenBehavior for HistoryScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let has_filter = self.filter_active || !self.filter.is_empty();
        let filter_height = if has_filter { 1 } else { 0 };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(filter_height),
                Constraint::Min(0), // table
                Constraint::Length(1), // footer
            ])
            .split(area);

        // Title bar
        let title = self.title_line();
        let title_widget = Paragraph::new(title).style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(title_widget, chunks[0]);

        // Filter bar
        if has_filter {
            let filter_text = format!("/{}", self.filter);
            let filter_style = if self.filter_active {
                Style::default().fg(theme::ACCENT).bg(theme::HEADER_BG)
            } else {
                Style::default().fg(theme::TEXT_DIM).bg(theme::HEADER_BG)
            };
            let filter_widget = Paragraph::new(filter_text).style(filter_style);
            f.render_widget(filter_widget, chunks[1]);
        }

        // Table
        let header = Row::new(vec![
            Cell::from("Date"),
            Cell::from("Project"),
            Cell::from("Branch"),
            Cell::from("Summary"),
        ])
        .style(theme::style_header())
        .height(1);

        let table_rows: Vec<Row> = self
            .displayed
            .iter()
            .map(|entry| {
                let proj = Self::shorten_project(&entry.project);
                Row::new(vec![
                    Cell::from(Span::styled(&*entry.date, Style::default().fg(theme::TEXT))),
                    Cell::from(Span::styled(
                        truncate_end(&proj, 40),
                        Style::default().fg(theme::TEXT),
                    )),
                    Cell::from(Span::styled(
                        truncate_end(&entry.branch, 20),
                        Style::default().fg(theme::TEXT),
                    )),
                    Cell::from(Span::styled(
                        truncate_end(entry.summary(), 50),
                        Style::default().fg(theme::TEXT),
                    )),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(16),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(40),
        ];

        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(theme::style_cursor())
            .style(theme::style_default())
            .block(Block::default().style(theme::style_default()));

        let mut state = self.table_state.clone();
        f.render_stateful_widget(table, chunks[2], &mut state);

        // Footer
        let footer = self.render_footer();
        f.render_widget(footer, chunks[3]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return ScreenAction::None;
        };

        // Filter mode
        if self.filter_active {
            return self.handle_filter_key(*code);
        }

        match code {
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild();
                    ScreenAction::None
                } else {
                    ScreenAction::Pop
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor(1);
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor(-1);
                ScreenAction::None
            }
            KeyCode::Char('t') => {
                if self.scope_paths.is_some() {
                    self.scoped = !self.scoped;
                    self.rebuild();
                }
                ScreenAction::None
            }
            KeyCode::Char('d') => {
                self.sort(SortMode::Date);
                ScreenAction::None
            }
            KeyCode::Char('p') => {
                self.sort(SortMode::Project);
                ScreenAction::None
            }
            KeyCode::Char('b') => {
                self.sort(SortMode::Branch);
                ScreenAction::None
            }
            KeyCode::Char('s') => {
                self.sort(SortMode::Summary);
                ScreenAction::None
            }
            KeyCode::Char('/') => {
                self.activate_filter();
                ScreenAction::None
            }
            KeyCode::Enter => self.action_resume(manager),
            _ => ScreenAction::None,
        }
    }

    fn is_modal(&self) -> bool {
        false
    }
}
