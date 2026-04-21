// trellis/src/tui/session_list.rs

use std::collections::{HashMap, HashSet};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use md5::{Md5, Digest};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::claude_session;
use crate::fuzzy::fuzzy_match;
use crate::manager::Manager;
use crate::models::{Repo, SessionInfo};
use crate::switch::SwitchAction;
use crate::tmux::{self, TmuxWindow};
use crate::utils::truncate_end;

use super::theme;
use super::{ActionResult, Screen, ScreenAction, ScreenBehavior};

/// Identifies what a row represents.
#[derive(Debug, Clone)]
enum RowKind {
    Session { row_key: String },
    Window { session_name: String, window_index: i64 },
}

/// Pre-built data for a single display row.
#[derive(Debug, Clone)]
struct RowData {
    kind: RowKind,
    col_session: Vec<StyledSegment>,
    col_repo: Vec<StyledSegment>,
    col_branch: Vec<StyledSegment>,
}

/// A segment of text with a style, used to build `Span`s at render time.
#[derive(Debug, Clone)]
struct StyledSegment {
    text: String,
    style: Style,
}

impl StyledSegment {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style }
    }
    fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), style: Style::default().fg(theme::TEXT) }
    }
    fn dim(text: impl Into<String>) -> Self {
        Self { text: text.into(), style: theme::style_dim() }
    }
}

fn segments_to_line(segs: &[StyledSegment]) -> Line<'_> {
    Line::from(
        segs.iter()
            .map(|s| Span::styled(s.text.as_str(), s.style))
            .collect::<Vec<_>>(),
    )
}

// --------------------------------------------------------------------------
// Repo color assignment (port of _assign_repo_colors)
// --------------------------------------------------------------------------

fn assign_repo_colors(repo_names: &HashSet<String>) -> HashMap<String, Color> {
    let colors = &theme::REPO_COLORS;
    let n = colors.len();
    let mut assignment: HashMap<String, Color> = HashMap::new();
    let mut used: HashSet<usize> = HashSet::new();

    let mut sorted: Vec<&String> = repo_names.iter().collect();
    sorted.sort();

    for name in sorted {
        let mut hasher = Md5::new();
        hasher.update(name.as_bytes());
        let digest = hasher.finalize();
        let preferred = {
            let val = u128::from_be_bytes(digest.into());
            (val % n as u128) as usize
        };
        let mut assigned = false;
        for i in 0..n {
            let candidate = (preferred + i) % n;
            if !used.contains(&candidate) {
                assignment.insert(name.clone(), colors[candidate]);
                used.insert(candidate);
                assigned = true;
                break;
            }
        }
        if !assigned {
            // All slots exhausted — collide on preferred
            assignment.insert(name.clone(), colors[preferred]);
        }
    }

    assignment
}

// --------------------------------------------------------------------------
// Auto-rename helper
// --------------------------------------------------------------------------

fn try_rename_claude_window(session_name: &str, win: &mut TmuxWindow) {
    let session_id = match claude_session::get_session_id(&win.pane_pid) {
        Some(id) => id,
        None => return,
    };
    let msg = match claude_session::get_first_user_message(&session_id) {
        Some(m) => m,
        None => return,
    };
    let name = claude_session::summarize_message(&msg, 4);
    if tmux::rename_window(session_name, win.index, &name).is_ok() {
        win.name = name;
    }
}

fn is_version_number(s: &str) -> bool {
    // Matches patterns like "1.2.3"
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 3 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

// --------------------------------------------------------------------------
// SessionListScreen
// --------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum ConfirmAction {
    DeleteSession { session_id: i64 },
    KillTab { session_name: String, window_index: i64 },
    KillSession { session_name: String },
}

#[derive(Debug)]
enum InlineMode {
    None,
    Confirm { message: String, action: ConfirmAction },
    Rename { input: String, cursor: usize, is_session: bool, session_id: Option<i64>, session_name: String, window_index: Option<i64> },
    Review { input: String, cursor: usize, repo_index: usize, repos: Vec<Repo> },
    NewTab { input: String, cursor: usize, session_id: i64, session_name: String },
    ActionMenu { items: Vec<(String, String)>, cursor: usize },
}

pub struct SessionListScreen {
    sessions: Vec<SessionInfo>,
    repos: HashMap<i64, Repo>,
    expanded: HashSet<String>,
    filter: String,
    filter_active: bool,
    cursor: usize,
    rows: Vec<RowData>,
    table_state: TableState,
    inline_mode: InlineMode,
    status_message: Option<String>,
}

impl SessionListScreen {
    pub fn new(manager: &Manager) -> Self {
        let mut screen = Self {
            sessions: Vec::new(),
            repos: HashMap::new(),
            expanded: HashSet::new(),
            filter: String::new(),
            filter_active: false,
            cursor: 0,
            rows: Vec::new(),
            table_state: TableState::default(),
            inline_mode: InlineMode::None,
            status_message: None,
        };
        screen.refresh(manager);
        screen
    }

    // ------------------------------------------------------------------
    // Data refresh
    // ------------------------------------------------------------------

    fn refresh(&mut self, manager: &Manager) {
        self.refresh_with_restore(manager, None);
    }

    fn refresh_with_restore(&mut self, manager: &Manager, restore_key: Option<&str>) {
        self.repos = manager.get_repos().into_iter().filter_map(|r| {
            r.id.map(|id| (id, r))
        }).collect();

        self.sessions = self.sorted_sessions(manager);
        let all_windows = tmux::list_all_windows();
        self.rows = self.build_rows(&all_windows, manager);

        // Restore cursor position
        if let Some(key) = restore_key {
            self.cursor = self.rows.iter().position(|r| self.row_key(r) == key).unwrap_or(0);
        } else if self.cursor >= self.rows.len() && !self.rows.is_empty() {
            self.cursor = self.rows.len() - 1;
        }
        self.table_state.select(if self.rows.is_empty() { None } else { Some(self.cursor) });
    }

    fn row_key(&self, row: &RowData) -> String {
        match &row.kind {
            RowKind::Session { row_key } => row_key.clone(),
            RowKind::Window { session_name, window_index } => {
                format!("win:{}:{}", session_name, window_index)
            }
        }
    }

    fn current_row_key(&self) -> Option<String> {
        self.rows.get(self.cursor).map(|r| self.row_key(r))
    }

    fn is_child_row(key: &str) -> bool {
        key.starts_with("win:")
    }

    // ------------------------------------------------------------------
    // Sorting + filtering
    // ------------------------------------------------------------------

    fn sorted_sessions(&self, manager: &Manager) -> Vec<SessionInfo> {
        let mut sessions = manager.list_sessions();

        if !self.filter.is_empty() {
            let mut scored: Vec<(SessionInfo, i32)> = Vec::new();
            for session in sessions {
                let repo = session.repo_id.and_then(|id| self.repos.get(&id));
                let candidates = [
                    session.name.as_str().to_string(),
                    repo.map(|r| r.name.clone()).unwrap_or_default(),
                    session.base_branch.clone().unwrap_or_default(),
                ];
                let mut best: Option<i32> = None;
                for c in &candidates {
                    if let Some(s) = fuzzy_match(&self.filter, c) {
                        if best.is_none() || s < best.unwrap() {
                            best = Some(s);
                        }
                    }
                }
                if let Some(score) = best {
                    scored.push((session, score));
                }
            }
            scored.sort_by_key(|x| x.1);
            return scored.into_iter().map(|(s, _)| s).collect();
        }

        // No filter: main pinned, then recency, then alphabetical
        sessions.sort_by(|a, b| {
            let a_main = if a.name == "main" { 0 } else { 1 };
            let b_main = if b.name == "main" { 0 } else { 1 };
            a_main.cmp(&b_main)
                .then_with(|| {
                    let a_has = if a.last_selected_at.is_some() { 0 } else { 1 };
                    let b_has = if b.last_selected_at.is_some() { 0 } else { 1 };
                    a_has.cmp(&b_has)
                })
                .then_with(|| {
                    // Reverse chronological for last_selected_at
                    let a_ts = a.last_selected_at.as_deref().unwrap_or("");
                    let b_ts = b.last_selected_at.as_deref().unwrap_or("");
                    b_ts.cmp(a_ts)
                })
                .then_with(|| a.name.cmp(&b.name))
        });
        sessions
    }

    // ------------------------------------------------------------------
    // Row building
    // ------------------------------------------------------------------

    fn build_rows(&self, all_windows: &HashMap<String, Vec<TmuxWindow>>, manager: &Manager) -> Vec<RowData> {
        let all_repo_names: HashSet<String> = self.sessions.iter()
            .filter_map(|s| s.repo_id.and_then(|id| self.repos.get(&id)).map(|r| r.name.clone()))
            .collect();
        let repo_colors = assign_repo_colors(&all_repo_names);

        let mut rows = Vec::new();
        let mut last_repo_name: Option<String> = None;

        for session in &self.sessions {
            let repo = session.repo_id.and_then(|id| self.repos.get(&id));
            let repo_name = repo.map(|r| r.name.as_str()).unwrap_or("");
            let branch = session.base_branch.as_deref().unwrap_or("-");
            let base = repo.map(|r| r.default_branch.as_str()).unwrap_or("-");
            let windows_str = session.windows.map(|w| w.to_string()).unwrap_or_else(|| "-".to_string());
            let color = repo_colors.get(repo_name).copied().unwrap_or(theme::TEXT_DIM);

            let row_key = if let Some(id) = session.id {
                id.to_string()
            } else {
                format!("unmanaged:{}", session.name)
            };

            let expanded = self.expanded.contains(&row_key);
            let can_expand = session.live;

            // Build session column
            let mut col_session = Vec::new();

            // Status indicator
            if session.attached {
                col_session.push(StyledSegment::new("●", Style::default().fg(theme::GREEN)));
            } else if !session.managed {
                col_session.push(StyledSegment::dim("◇"));
            } else if session.live {
                col_session.push(StyledSegment::new("○", Style::default().fg(theme::BLUE)));
            } else {
                col_session.push(StyledSegment::plain(" "));
            }

            col_session.push(StyledSegment::plain(" "));

            // Expand arrow
            if expanded {
                col_session.push(StyledSegment::plain("▾"));
            } else if can_expand {
                col_session.push(StyledSegment::plain("▸"));
            } else {
                col_session.push(StyledSegment::plain(" "));
            }

            col_session.push(StyledSegment::plain(" "));

            // Session name
            if session.attached {
                col_session.push(StyledSegment::new(&session.name, Style::default().fg(theme::GREEN)));
            } else {
                col_session.push(StyledSegment::plain(&session.name));
            }

            // Window count
            if windows_str != "-" {
                col_session.push(StyledSegment::dim(format!(" ({})", windows_str)));
            }

            // Build repo column
            let mut col_repo = Vec::new();
            if !self.filter.is_empty() || (last_repo_name.as_deref() != Some(repo_name) && !repo_name.is_empty()) {
                if !repo_name.is_empty() {
                    col_repo.push(StyledSegment::new(
                        truncate_end(repo_name, 20),
                        Style::default().fg(color),
                    ));
                } else {
                    col_repo.push(StyledSegment::dim("-"));
                }
            } else if !repo_name.is_empty() {
                col_repo.push(StyledSegment::dim(truncate_end(repo_name, 20)));
            } else {
                col_repo.push(StyledSegment::dim("-"));
            }
            last_repo_name = if repo_name.is_empty() { None } else { Some(repo_name.to_string()) };

            // Build branch column
            let col_branch = if branch == base || branch == "-" {
                vec![StyledSegment::plain(branch)]
            } else {
                vec![
                    StyledSegment::plain(base),
                    StyledSegment::dim(" → "),
                    StyledSegment::plain(branch),
                ]
            };

            rows.push(RowData {
                kind: RowKind::Session { row_key: row_key.clone() },
                col_session,
                col_repo,
                col_branch,
            });

            // Expanded child rows
            if expanded && session.live {
                let mut tmux_windows = all_windows.get(&session.name).cloned().unwrap_or_default();

                // Build worktree lookup
                let mut wt_by_path: HashMap<String, String> = HashMap::new();
                if session.managed {
                    if let Some(id) = session.id {
                        for wt in manager.get_worktrees_for_session(id) {
                            wt_by_path.insert(wt.path.clone(), wt.branch.clone());
                        }
                    }
                }

                let win_count = tmux_windows.len();
                for (i, win) in tmux_windows.iter_mut().enumerate() {
                    let is_last = i == win_count - 1;
                    let prefix = if is_last { "└" } else { "├" };
                    let wt_branch = wt_by_path.get(&win.path).or_else(|| {
                        wt_by_path.iter().find_map(|(path, branch)| {
                            if win.path.starts_with(path) { Some(branch) } else { None }
                        })
                    });
                    let is_claude = !win.command.is_empty() && is_version_number(&win.command);

                    // Auto-rename version-numbered claude windows
                    if is_claude && is_version_number(&win.name) {
                        try_rename_claude_window(&session.name, win);
                    }

                    let mut col_repo_child = Vec::new();
                    if is_claude {
                        let pane_text = tmux::capture_pane(
                            &format!("{}:{}", session.name, win.index),
                            8,
                        );
                        let state = claude_session::classify_pane(&pane_text);
                        match state {
                            "working" => {
                                col_repo_child.push(StyledSegment::new("✦ working…", Style::default().fg(theme::ORANGE)));
                            }
                            "prompting" => {
                                col_repo_child.push(StyledSegment::new("✦ needs input", Style::default().fg(theme::RED)));
                            }
                            "idle" => {
                                col_repo_child.push(StyledSegment::dim("✦ idle"));
                            }
                            _ => {
                                col_repo_child.push(StyledSegment::new("✦ claude", Style::default().fg(theme::ORANGE)));
                            }
                        }
                    } else if !win.command.is_empty() && win.command != "zsh" {
                        col_repo_child.push(StyledSegment::new(
                            &win.command,
                            Style::default().fg(theme::TEXT).add_modifier(Modifier::ITALIC),
                        ));
                    }

                    let col_branch_child = if let Some(branch) = wt_branch {
                        vec![
                            StyledSegment::dim("wt: "),
                            StyledSegment::plain(branch),
                        ]
                    } else {
                        vec![StyledSegment::dim(truncate_end(&win.path, 30))]
                    };

                    let col_session_child = vec![
                        StyledSegment::plain("      "),
                        StyledSegment::dim(prefix),
                        StyledSegment::plain(" "),
                        StyledSegment::dim(&win.name),
                    ];

                    rows.push(RowData {
                        kind: RowKind::Window {
                            session_name: session.name.clone(),
                            window_index: win.index,
                        },
                        col_session: col_session_child,
                        col_repo: col_repo_child,
                        col_branch: col_branch_child,
                    });
                }
            }
        }

        rows
    }

    // ------------------------------------------------------------------
    // Session lookup
    // ------------------------------------------------------------------

    fn session_for_row_key(&self, row_key: &str) -> Option<&SessionInfo> {
        if row_key.starts_with("unmanaged:") {
            let name = &row_key["unmanaged:".len()..];
            return self.sessions.iter().find(|s| s.name == name);
        }
        if let Ok(id) = row_key.parse::<i64>() {
            return self.sessions.iter().find(|s| s.id == Some(id));
        }
        None
    }

    pub fn current_session(&self) -> Option<&SessionInfo> {
        let key = self.current_row_key()?;
        if Self::is_child_row(&key) {
            return None;
        }
        self.session_for_row_key(&key)
    }

    fn touch_by_name(&self, session_name: &str, manager: &Manager) {
        if let Some(session) = self.sessions.iter().find(|s| s.name == session_name && s.managed && s.id.is_some()) {
            manager.touch_session(session.id.unwrap());
        }
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    fn cursor_down(&mut self) {
        if !self.rows.is_empty() && self.cursor < self.rows.len() - 1 {
            self.cursor += 1;
            self.table_state.select(Some(self.cursor));
        }
    }

    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.table_state.select(Some(self.cursor));
        }
    }

    fn handle_enter(&mut self, manager: &mut Manager) -> ScreenAction {
        let key = match self.current_row_key() {
            Some(k) => k,
            None => return ScreenAction::None,
        };

        if key.starts_with("win:") {
            let parts: Vec<&str> = key.splitn(3, ':').collect();
            if parts.len() == 3 {
                let session_name = parts[1];
                let window_index: i64 = parts[2].parse().unwrap_or(0);
                self.touch_by_name(session_name, manager);
                return ScreenAction::Switch(SwitchAction::Window {
                    session: session_name.to_string(),
                    window: window_index,
                });
            }
        }

        if Self::is_child_row(&key) {
            return ScreenAction::None;
        }

        self.switch_to_session(&key, manager)
    }

    fn switch_to_session(&self, row_key: &str, manager: &Manager) -> ScreenAction {
        let session = match self.session_for_row_key(row_key) {
            Some(s) => s,
            None => return ScreenAction::None,
        };
        if session.managed {
            if let Some(id) = session.id {
                manager.touch_session(id);
            }
        }
        ScreenAction::Switch(SwitchAction::Session {
            target: session.name.clone(),
        })
    }

    fn handle_expand(&mut self, manager: &Manager) {
        let key = match self.current_row_key() {
            Some(k) => k,
            None => return,
        };
        if Self::is_child_row(&key) {
            return;
        }
        let session = match self.session_for_row_key(&key) {
            Some(s) => s,
            None => return,
        };
        if !session.live {
            return;
        }
        if !self.expanded.contains(&key) {
            self.expanded.insert(key.clone());
            self.refresh_with_restore(manager, Some(&key));
        }
    }

    fn handle_collapse(&mut self, manager: &Manager) {
        let key = match self.current_row_key() {
            Some(k) => k,
            None => return,
        };
        // If on a child row, collapse the parent
        if Self::is_child_row(&key) {
            // Find the parent session key by looking backward in rows
            if let Some(cursor) = self.table_state.selected() {
                for i in (0..cursor).rev() {
                    if let RowKind::Session { row_key } = &self.rows[i].kind {
                        let key = row_key.clone();
                        self.expanded.remove(&key);
                        self.refresh_with_restore(manager, Some(&key));
                        return;
                    }
                }
            }
            return;
        }
        if self.expanded.contains(&key) {
            self.expanded.remove(&key);
            self.refresh_with_restore(manager, Some(&key));
        }
    }

    // ------------------------------------------------------------------
    // Filter
    // ------------------------------------------------------------------

    fn activate_filter(&mut self) {
        self.filter_active = true;
        self.filter.clear();
    }

    fn dismiss_filter(&mut self, manager: &Manager) {
        self.filter_active = false;
        if !self.filter.is_empty() {
            self.filter.clear();
            self.refresh(manager);
        }
    }

    fn handle_filter_key(&mut self, key: KeyCode, manager: &Manager) -> ScreenAction {
        match key {
            KeyCode::Esc => {
                self.dismiss_filter(manager);
                ScreenAction::None
            }
            KeyCode::Enter => {
                // Dismiss filter input, keep filter text active
                self.filter_active = false;
                ScreenAction::None
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.refresh(manager);
                ScreenAction::None
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.refresh(manager);
                ScreenAction::None
            }
            _ => ScreenAction::None,
        }
    }

    // ------------------------------------------------------------------
    // Rendering helpers
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    fn action_new_tab(&mut self, _manager: &Manager) -> ScreenAction {
        // Find the managed session for the current row
        let session = match self.current_session() {
            Some(s) if s.managed => s.clone(),
            _ => {
                // Try parent session if on a child row
                if let Some(key) = self.current_row_key() {
                    if key.starts_with("win:") {
                        let parts: Vec<&str> = key.splitn(3, ':').collect();
                        if parts.len() >= 2 {
                            let session_name = parts[1];
                            if let Some(s) = self.sessions.iter().find(|s| s.name == session_name && s.managed) {
                                let s = s.clone();
                                self.inline_mode = InlineMode::NewTab {
                                    input: String::new(),
                                    cursor: 0,
                                    session_id: s.id.unwrap(),
                                    session_name: s.name.clone(),
                                };
                                return ScreenAction::None;
                            }
                        }
                    }
                }
                self.status_message = Some("No managed session selected.".to_string());
                return ScreenAction::None;
            }
        };
        if let Some(id) = session.id {
            self.inline_mode = InlineMode::NewTab {
                input: String::new(),
                cursor: 0,
                session_id: id,
                session_name: session.name.clone(),
            };
        }
        ScreenAction::None
    }

    fn action_rename(&mut self, _manager: &Manager) -> ScreenAction {
        let row_key = match self.current_row_key() {
            Some(k) => k,
            None => return ScreenAction::None,
        };

        // Rename tab
        if row_key.starts_with("win:") {
            let parts: Vec<&str> = row_key.splitn(3, ':').collect();
            if parts.len() == 3 {
                let session_name = parts[1].to_string();
                let window_index: i64 = parts[2].parse().unwrap_or(0);
                let windows = tmux::list_windows(&session_name);
                if let Some(win) = windows.iter().find(|w| w.index == window_index) {
                    self.inline_mode = InlineMode::Rename {
                        input: win.name.clone(),
                        cursor: win.name.len(),
                        is_session: false,
                        session_id: None,
                        session_name,
                        window_index: Some(window_index),
                    };
                }
            }
            return ScreenAction::None;
        }

        // Rename session
        let session = match self.current_session() {
            Some(s) => s.clone(),
            None => return ScreenAction::None,
        };
        if session.managed {
            self.inline_mode = InlineMode::Rename {
                input: session.name.clone(),
                cursor: session.name.len(),
                is_session: true,
                session_id: session.id,
                session_name: session.name.clone(),
                window_index: None,
            };
        }
        ScreenAction::None
    }

    fn action_review(&mut self, manager: &Manager) -> ScreenAction {
        let mut repos = manager.get_repos();
        if repos.is_empty() {
            self.status_message = Some("No repos configured.".to_string());
            return ScreenAction::None;
        }
        // Sort by most recently active session
        let sessions = manager.get_sessions();
        let mut repo_last_active: HashMap<i64, String> = HashMap::new();
        for s in &sessions {
            if let Some(ref ts) = s.last_selected_at {
                let id = s.repo_id;
                if !repo_last_active.contains_key(&id) || ts > repo_last_active.get(&id).unwrap() {
                    repo_last_active.insert(id, ts.clone());
                }
            }
        }
        repos.sort_by(|a, b| {
            let a_ts = a.id.and_then(|id| repo_last_active.get(&id));
            let b_ts = b.id.and_then(|id| repo_last_active.get(&id));
            b_ts.cmp(&a_ts)
        });

        self.inline_mode = InlineMode::Review {
            input: String::new(),
            cursor: 0,
            repo_index: 0,
            repos,
        };
        ScreenAction::None
    }

    fn action_delete(&mut self, _manager: &Manager) -> ScreenAction {
        let key = match self.current_row_key() {
            Some(k) => k,
            None => return ScreenAction::None,
        };

        // Kill a tab
        if key.starts_with("win:") {
            let parts: Vec<&str> = key.splitn(3, ':').collect();
            if parts.len() == 3 {
                let session_name = parts[1].to_string();
                let window_index: i64 = parts[2].parse().unwrap_or(0);
                self.inline_mode = InlineMode::Confirm {
                    message: format!("Kill tab {} in '{}'? [y] Yes  [n] No", window_index, session_name),
                    action: ConfirmAction::KillTab { session_name, window_index },
                };
                return ScreenAction::None;
            }
        }

        if Self::is_child_row(&key) {
            return ScreenAction::None;
        }

        // Delete a session
        let session = match self.session_for_row_key(&key) {
            Some(s) => s.clone(),
            None => return ScreenAction::None,
        };

        if session.managed {
            let msg = if session.live {
                format!("Delete session '{}' (tmux session will be killed)? [y] Yes  [n] No", session.name)
            } else {
                format!("Delete session '{}'? [y] Yes  [n] No", session.name)
            };
            self.inline_mode = InlineMode::Confirm {
                message: msg,
                action: ConfirmAction::DeleteSession { session_id: session.id.unwrap() },
            };
            ScreenAction::None
        } else {
            self.inline_mode = InlineMode::Confirm {
                message: format!("Kill tmux session '{}'? [y] Yes  [n] No", session.name),
                action: ConfirmAction::KillSession { session_name: session.name.clone() },
            };
            ScreenAction::None
        }
    }

    fn action_action_menu(&mut self, _manager: &Manager) -> ScreenAction {
        let key = match self.current_row_key() {
            Some(k) => k,
            None => return ScreenAction::None,
        };

        // Tab-level actions — no items remain after removing rename-tab
        if key.starts_with("win:") {
            self.status_message = Some("No actions available for tabs.".to_string());
            return ScreenAction::None;
        }

        let session = match self.current_session() {
            Some(s) => s.clone(),
            None => return ScreenAction::None,
        };

        let mut items: Vec<(String, String)> = Vec::new();
        if session.managed {
            // "rename" removed — use `r` directly
            items.push(("branch".to_string(), "Change branch".to_string()));
            if session.live {
                items.push(("claude".to_string(), "Launch claude".to_string()));
            }
        } else if session.live {
            items.push(("adopt".to_string(), "Adopt session".to_string()));
        }

        if items.is_empty() {
            self.status_message = Some("No actions available.".to_string());
            return ScreenAction::None;
        }

        self.inline_mode = InlineMode::ActionMenu {
            items,
            cursor: 0,
        };
        ScreenAction::None
    }

    fn action_history(&self, manager: &Manager) -> ScreenAction {
        let session = self.current_session();
        let mut scope_paths = None;
        let mut scope_label = None;

        if let Some(s) = session {
            if s.managed {
                if let Some(repo_id) = s.repo_id {
                    if let Some(repo) = self.repos.get(&repo_id) {
                        let mut paths = vec![repo.path.clone()];
                        if let Some(id) = s.id {
                            for wt in manager.get_worktrees_for_session(id) {
                                paths.push(wt.path.clone());
                            }
                            // Include worktrees root for this repo
                            let wt_root = manager.worktrees_dir().join(&repo.name);
                            paths.push(wt_root.to_string_lossy().to_string());
                        }
                        scope_paths = Some(paths);
                        scope_label = Some(s.name.clone());
                    }
                }
            }
        }

        let screen = super::history::HistoryScreen::new(manager, scope_paths, scope_label);
        ScreenAction::Push(Screen::History(screen))
    }

    fn action_cleanup(&self, manager: &Manager) -> ScreenAction {
        let screen = super::cleanup::CleanupScreen::new(manager);
        ScreenAction::Push(Screen::Cleanup(screen))
    }

    fn action_settings(&self, manager: &Manager) -> ScreenAction {
        let screen = super::settings::SettingsScreen::new(manager);
        ScreenAction::Push(Screen::Settings(screen))
    }

    fn action_help(&self) -> ScreenAction {
        ScreenAction::Push(Screen::Help(super::help::HelpScreen))
    }

    // ------------------------------------------------------------------
    // Child result handlers
    // ------------------------------------------------------------------

    fn handle_action_picked(&mut self, key: Option<String>, manager: &Manager) -> ScreenAction {
        let key = match key {
            Some(k) => k,
            None => return ScreenAction::None,
        };
        let session = self.current_session().cloned();

        let session = match session {
            Some(s) => s,
            None => return ScreenAction::None,
        };

        match key.as_str() {
            "branch" if session.managed => {
                if let Some(id) = session.id {
                    let screen = super::edit_branch::EditBranchScreen::new(
                        manager,
                        id,
                        session.name.clone(),
                    );
                    ScreenAction::Push(Screen::EditBranch(screen))
                } else {
                    ScreenAction::None
                }
            }
            "claude" if session.live => {
                let _ = tmux::new_window(&session.name, "claude", None);
                let target = format!("{}:claude", session.name);
                tmux::send_keys(&target, &["claude", "Enter"]);
                if session.managed {
                    if let Some(id) = session.id {
                        manager.touch_session(id);
                    }
                }
                crate::switch::write_switch(&SwitchAction::Session {
                    target: session.name.clone(),
                });
                ScreenAction::Quit
            }
            "adopt" if !session.managed => {
                let screen = super::adopt_session::AdoptSessionScreen::new(
                    manager,
                    session.name.clone(),
                );
                ScreenAction::Push(Screen::AdoptSession(screen))
            }
            _ => ScreenAction::None,
        }
    }

    fn render_footer<'a>(&self) -> Paragraph<'a> {
        let bindings = vec![
            ("q", "Quit"),
            ("/", "Filter"),
            ("enter", "Switch"),
            ("h/l", "Collapse/Expand"),
            ("n", "New"),
            ("t", "Tab"),
            ("r", "Rename"),
            ("R", "Review"),
            ("d", "Delete"),
            ("H", "History"),
            (".", "Actions"),
            ("c", "Cleanup"),
            ("S", "Settings"),
            ("?", "Help"),
        ];

        let mut spans = Vec::new();
        for (i, (key, label)) in bindings.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", theme::style_footer()));
            }
            spans.push(Span::styled(*key, theme::style_footer_key()));
            spans.push(Span::styled(format!(" {}", label), theme::style_footer()));
        }

        Paragraph::new(Line::from(spans))
            .style(theme::style_footer())
    }

    fn handle_rename_key(&mut self, code: KeyCode, modifiers: crossterm::event::KeyModifiers, manager: &mut Manager) -> ScreenAction {
        let (input, _cursor, is_session, session_id, session_name, window_index) = match &mut self.inline_mode {
            InlineMode::Rename { input, cursor, is_session, session_id, session_name, window_index } => {
                (input, cursor, *is_session, *session_id, session_name.clone(), *window_index)
            }
            _ => return ScreenAction::None,
        };

        match code {
            KeyCode::Enter => {
                let name = input.trim().to_string();
                if name.is_empty() {
                    self.status_message = Some("Name cannot be empty.".to_string());
                    self.inline_mode = InlineMode::None;
                    return ScreenAction::None;
                }
                if is_session {
                    let sanitized = tmux::sanitize_session_name(&name);
                    if let Some(id) = session_id {
                        if manager.get_session_by_name(&sanitized).is_some() && sanitized != session_name {
                            self.status_message = Some(format!("Session '{}' already exists.", sanitized));
                            return ScreenAction::None;
                        }
                        if let Err(e) = manager.rename_session(id, &sanitized) {
                            self.status_message = Some(format!("Error: {}", e));
                            self.inline_mode = InlineMode::None;
                            return ScreenAction::None;
                        }
                    }
                } else if let Some(win_idx) = window_index {
                    if let Err(e) = tmux::rename_window(&session_name, win_idx, &name) {
                        self.status_message = Some(format!("Error: {}", e));
                        self.inline_mode = InlineMode::None;
                        return ScreenAction::None;
                    }
                }
                self.status_message = Some("Renamed.".to_string());
                self.inline_mode = InlineMode::None;
                self.refresh(manager);
                ScreenAction::None
            }
            KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            _ => {
                use super::rename::input_handle_key;
                if let InlineMode::Rename { input, cursor, .. } = &mut self.inline_mode {
                    input_handle_key(input, cursor, code, modifiers);
                }
                ScreenAction::None
            }
        }
    }

    fn handle_confirm_key(&mut self, code: KeyCode, manager: &mut Manager) -> ScreenAction {
        let action = match &self.inline_mode {
            InlineMode::Confirm { action, .. } => action.clone(),
            _ => return ScreenAction::None,
        };

        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                match action {
                    ConfirmAction::DeleteSession { session_id } => {
                        let _ = manager.delete_session(session_id, false);
                    }
                    ConfirmAction::KillTab { session_name, window_index } => {
                        let _ = tmux::kill_window(&session_name, window_index);
                    }
                    ConfirmAction::KillSession { session_name } => {
                        let _ = tmux::kill_session(&session_name);
                    }
                }
                self.inline_mode = InlineMode::None;
                self.refresh(manager);
                ScreenAction::None
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            _ => ScreenAction::None,
        }
    }

    fn handle_review_key(&mut self, code: KeyCode, modifiers: crossterm::event::KeyModifiers, manager: &mut Manager) -> ScreenAction {
        match code {
            KeyCode::Tab => {
                if let InlineMode::Review { repo_index, repos, .. } = &mut self.inline_mode {
                    if repos.len() > 1 {
                        *repo_index = (*repo_index + 1) % repos.len();
                    }
                }
                ScreenAction::None
            }
            KeyCode::Enter => {
                let (input, repo) = match &self.inline_mode {
                    InlineMode::Review { input, repo_index, repos, .. } => {
                        (input.trim().to_string(), repos.get(*repo_index).cloned())
                    }
                    _ => return ScreenAction::None,
                };
                let repo = match repo {
                    Some(r) => r,
                    None => return ScreenAction::None,
                };
                if input.is_empty() {
                    self.status_message = Some("Please enter a PR number or branch name.".to_string());
                    return ScreenAction::None;
                }
                self.inline_mode = InlineMode::None;

                match manager.checkout_and_review(&repo.path, &input) {
                    Ok((session, _worktree_path)) => {
                        ScreenAction::Switch(SwitchAction::Session {
                            target: session.name,
                        })
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Error: {}", e));
                        ScreenAction::None
                    }
                }
            }
            KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            _ => {
                use super::rename::input_handle_key;
                if let InlineMode::Review { input, cursor, .. } = &mut self.inline_mode {
                    input_handle_key(input, cursor, code, modifiers);
                }
                ScreenAction::None
            }
        }
    }

    fn handle_new_tab_key(&mut self, code: KeyCode, modifiers: crossterm::event::KeyModifiers, manager: &mut Manager) -> ScreenAction {
        match code {
            KeyCode::Enter => {
                let (input, session_id, session_name) = match &self.inline_mode {
                    InlineMode::NewTab { input, session_id, session_name, .. } => {
                        (input.trim().to_string(), *session_id, session_name.clone())
                    }
                    _ => return ScreenAction::None,
                };
                if input.is_empty() {
                    self.status_message = Some("Branch name cannot be empty.".to_string());
                    return ScreenAction::None;
                }
                self.inline_mode = InlineMode::None;
                match manager.add_tab(session_id, &input) {
                    Ok(_) => {
                        tmux::send_keys(
                            &format!("{}:{}", session_name, input),
                            &["claude", "Enter"],
                        );
                        ScreenAction::Switch(SwitchAction::Session {
                            target: session_name,
                        })
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Error: {}", e));
                        ScreenAction::None
                    }
                }
            }
            KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            _ => {
                use super::rename::input_handle_key;
                if let InlineMode::NewTab { input, cursor, .. } = &mut self.inline_mode {
                    input_handle_key(input, cursor, code, modifiers);
                }
                ScreenAction::None
            }
        }
    }

    fn handle_action_menu_key(&mut self, code: KeyCode, manager: &Manager) -> ScreenAction {
        let (items, menu_cursor) = match &mut self.inline_mode {
            InlineMode::ActionMenu { items, cursor } => (items.clone(), cursor),
            _ => return ScreenAction::None,
        };

        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let InlineMode::ActionMenu { cursor, items } = &mut self.inline_mode {
                    if *cursor + 1 < items.len() {
                        *cursor += 1;
                    }
                }
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let InlineMode::ActionMenu { cursor, .. } = &mut self.inline_mode {
                    *cursor = cursor.saturating_sub(1);
                }
                ScreenAction::None
            }
            KeyCode::Enter => {
                let selected_key = items.get(*menu_cursor).map(|(k, _)| k.clone());
                self.inline_mode = InlineMode::None;
                if let Some(key) = selected_key {
                    return self.handle_action_picked(Some(key), manager);
                }
                ScreenAction::None
            }
            KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            _ => ScreenAction::None,
        }
    }
}

impl ScreenBehavior for SessionListScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        // Layout: optional filter at top, table in middle, footer at bottom
        let has_filter = self.filter_active || !self.filter.is_empty();
        let filter_height = if has_filter { 1 } else { 0 };

        let status_height = match &self.inline_mode {
            InlineMode::None => if self.status_message.is_some() { 1 } else { 0 },
            _ => 1,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(filter_height),
                Constraint::Min(0),
                Constraint::Length(status_height),
                Constraint::Length(1),
            ])
            .split(area);

        // Filter bar
        if has_filter {
            let filter_text = format!("/{}", self.filter);
            let filter_style = if self.filter_active {
                Style::default().fg(theme::ACCENT).bg(theme::HEADER_BG)
            } else {
                Style::default().fg(theme::TEXT_DIM).bg(theme::HEADER_BG)
            };
            let filter_widget = Paragraph::new(filter_text).style(filter_style);
            f.render_widget(filter_widget, chunks[0]);
        }

        // Build table rows
        let header = Row::new(vec![
            Cell::from("Session"),
            Cell::from("Repo"),
            Cell::from("Branch"),
        ])
        .style(theme::style_header())
        .height(1);

        let table_rows: Vec<Row> = self.rows.iter().map(|row| {
            Row::new(vec![
                Cell::from(segments_to_line(&row.col_session)),
                Cell::from(segments_to_line(&row.col_repo)),
                Cell::from(segments_to_line(&row.col_branch)),
            ])
        }).collect();

        let widths = [
            Constraint::Percentage(40),
            Constraint::Percentage(25),
            Constraint::Percentage(35),
        ];

        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(theme::style_cursor())
            .style(theme::style_default())
            .block(Block::default().style(theme::style_default()));

        let mut state = self.table_state.clone();
        f.render_stateful_widget(table, chunks[1], &mut state);

        // Status line
        match &self.inline_mode {
            InlineMode::None => {
                if let Some(ref msg) = self.status_message {
                    let status = Paragraph::new(msg.as_str())
                        .style(Style::default().fg(theme::TEXT_DIM).bg(theme::BG));
                    f.render_widget(status, chunks[2]);
                }
            }
            InlineMode::Confirm { message, .. } => {
                let status = Paragraph::new(message.as_str())
                    .style(Style::default().fg(theme::YELLOW).bg(theme::BG));
                f.render_widget(status, chunks[2]);
            }
            InlineMode::Rename { ref input, cursor: cursor_pos, .. } => {
                // Show input hint in status line
                let hint = Paragraph::new("[enter] confirm  [esc] cancel")
                    .style(Style::default().fg(theme::TEXT_DIM).bg(theme::BG));
                f.render_widget(hint, chunks[2]);

                // Overlay input on the current row's Session column
                // The session column has a prefix: "● ▸ " (status + expand = 4 chars)
                // We overlay the entire session column to cover the old text including "(N)" count
                let table_area = chunks[1];
                let selected = self.table_state.selected().unwrap_or(0);
                let scroll_offset = self.table_state.offset();
                let visible_row = selected - scroll_offset;
                let row_y = table_area.y + 1 + visible_row as u16; // +1 for header

                if row_y < table_area.y + table_area.height {
                    let session_col_width = table_area.width * 40 / 100;
                    let input_area = Rect::new(table_area.x, row_y, session_col_width, 1);

                    // Reconstruct the row prefix (status indicator + expand arrow)
                    // to keep the visual context while editing
                    let row_prefix = if let Some(idx) = self.table_state.selected() {
                        if idx < self.rows.len() {
                            // Extract the first 4 chars of the session column (status + arrow + spaces)
                            let segs = &self.rows[idx].col_session;
                            let mut prefix_spans: Vec<Span> = Vec::new();
                            let mut chars_taken = 0;
                            for seg in segs {
                                if chars_taken >= 4 {
                                    break;
                                }
                                let remaining = 4 - chars_taken;
                                let seg_chars: String = seg.text.chars().take(remaining).collect();
                                chars_taken += seg_chars.chars().count();
                                prefix_spans.push(Span::styled(seg_chars, seg.style.bg(theme::CURSOR_BG)));
                            }
                            prefix_spans
                        } else {
                            vec![Span::styled("    ", Style::default().bg(theme::CURSOR_BG))]
                        }
                    } else {
                        vec![Span::styled("    ", Style::default().bg(theme::CURSOR_BG))]
                    };

                    // Build the input display with cursor (cursor_pos is a byte index)
                    let before = &input[..*cursor_pos];
                    let cursor_char_len = input[*cursor_pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                    let cursor_display = if *cursor_pos < input.len() {
                        &input[*cursor_pos..*cursor_pos + cursor_char_len]
                    } else {
                        " "
                    };
                    let after = &input[(*cursor_pos + cursor_char_len).min(input.len())..];

                    let mut spans = row_prefix;
                    spans.push(Span::styled(before.to_string(), Style::default().fg(theme::TEXT).bg(theme::CURSOR_BG)));
                    spans.push(Span::styled(
                        cursor_display.to_string(),
                        Style::default().fg(theme::BG).bg(theme::ACCENT),
                    ));
                    spans.push(Span::styled(after.to_string(), Style::default().fg(theme::TEXT).bg(theme::CURSOR_BG)));

                    let input_widget = Paragraph::new(Line::from(spans))
                        .style(Style::default().bg(theme::CURSOR_BG));
                    f.render_widget(input_widget, input_area);
                }
            }
            InlineMode::Review { ref input, cursor: cursor_pos, repo_index, ref repos } => {
                let repo_name = repos.get(*repo_index).map(|r| r.name.as_str()).unwrap_or("?");
                let prefix = format!("Review ({}): ", repo_name);
                let suffix = if repos.len() > 1 { "  [tab] cycle repo  [esc] cancel" } else { "  [esc] cancel" };

                let before = &input[..*cursor_pos];
                let cursor_char_len = input[*cursor_pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                let cursor_display = if *cursor_pos < input.len() {
                    input[*cursor_pos..*cursor_pos + cursor_char_len].to_string()
                } else {
                    " ".to_string()
                };
                let after = &input[(*cursor_pos + cursor_char_len).min(input.len())..];

                let spans = vec![
                    Span::styled(&prefix, Style::default().fg(theme::ACCENT)),
                    Span::styled(before.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(cursor_display, Style::default().fg(theme::BG).bg(theme::ACCENT)),
                    Span::styled(after.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(suffix, Style::default().fg(theme::TEXT_DIM)),
                ];
                let status = Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::BG));
                f.render_widget(status, chunks[2]);
            }
            InlineMode::NewTab { ref input, cursor: cursor_pos, ref session_name, .. } => {
                let prefix = format!("New tab in '{}': ", session_name);
                let suffix = "  [enter] create  [esc] cancel";

                let before = &input[..*cursor_pos];
                let cursor_char_len = input[*cursor_pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                let cursor_display = if *cursor_pos < input.len() {
                    input[*cursor_pos..*cursor_pos + cursor_char_len].to_string()
                } else {
                    " ".to_string()
                };
                let after = &input[(*cursor_pos + cursor_char_len).min(input.len())..];

                let spans = vec![
                    Span::styled(&prefix, Style::default().fg(theme::ACCENT)),
                    Span::styled(before.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(cursor_display, Style::default().fg(theme::BG).bg(theme::ACCENT)),
                    Span::styled(after.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(suffix, Style::default().fg(theme::TEXT_DIM)),
                ];
                let status = Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::BG));
                f.render_widget(status, chunks[2]);
            }
            InlineMode::ActionMenu { ref items, cursor: menu_cursor } => {
                // Draw hint in status line
                let hint = Paragraph::new("[j/k] navigate  [enter] select  [esc] cancel")
                    .style(Style::default().fg(theme::TEXT_DIM).bg(theme::BG));
                f.render_widget(hint, chunks[2]);

                // Calculate dropdown position anchored to cursor row
                let table_area = chunks[1];
                let selected = self.table_state.selected().unwrap_or(0);
                let scroll_offset = self.table_state.offset();
                let visible_row = selected - scroll_offset;
                let anchor_y = table_area.y + 1 + visible_row as u16;

                let menu_height = items.len().min(8) as u16 + 2; // +2 for border
                let menu_width = items.iter().map(|(_, label)| label.len()).max().unwrap_or(10) as u16 + 6; // padding + border

                // Position: below if room, above if not
                let menu_y = if anchor_y + 1 + menu_height <= table_area.y + table_area.height {
                    anchor_y + 1
                } else {
                    anchor_y.saturating_sub(menu_height)
                };

                let menu_area = Rect::new(
                    table_area.x + 2,
                    menu_y,
                    menu_width.min(table_area.width.saturating_sub(4)),
                    menu_height.min(table_area.height),
                );

                // Clear area and draw menu
                f.render_widget(ratatui::widgets::Clear, menu_area);

                let menu_items: Vec<Line> = items.iter().enumerate().map(|(i, (_, label))| {
                    if i == *menu_cursor {
                        Line::from(Span::styled(
                            format!(" {} ", label),
                            Style::default().fg(Color::White).bg(theme::CURSOR_BG),
                        ))
                    } else {
                        Line::from(Span::styled(
                            format!(" {} ", label),
                            Style::default().fg(theme::TEXT),
                        ))
                    }
                }).collect();

                let menu_block = Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(Style::default().fg(theme::ACCENT))
                    .style(Style::default().bg(theme::HEADER_BG));

                let menu_widget = Paragraph::new(menu_items).block(menu_block);
                f.render_widget(menu_widget, menu_area);
            }
        }

        // Footer
        let footer = self.render_footer();
        f.render_widget(footer, chunks[3]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        // Handle mouse events
        if let Event::Mouse(mouse) = event {
            use crossterm::event::{MouseEventKind, MouseButton};
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.cursor_up();
                    return ScreenAction::None;
                }
                MouseEventKind::ScrollDown => {
                    self.cursor_down();
                    return ScreenAction::None;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    // Click to select row — offset by header row and filter bar
                    let header_offset = if self.filter_active { 2 } else { 1 };
                    let row = mouse.row as usize;
                    if row >= header_offset && row - header_offset < self.rows.len() {
                        self.cursor = row - header_offset;
                        self.table_state.select(Some(self.cursor));
                    }
                    return ScreenAction::None;
                }
                _ => return ScreenAction::None,
            }
        }

        let Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) = event else {
            return ScreenAction::None;
        };

        // If an inline mode is active, route to mode-specific handler
        match &self.inline_mode {
            InlineMode::None => {}
            InlineMode::Confirm { .. } => return self.handle_confirm_key(*code, manager),
            InlineMode::Rename { .. } => return self.handle_rename_key(*code, *modifiers, manager),
            InlineMode::Review { .. } => return self.handle_review_key(*code, *modifiers, manager),
            InlineMode::NewTab { .. } => return self.handle_new_tab_key(*code, *modifiers, manager),
            InlineMode::ActionMenu { .. } => return self.handle_action_menu_key(*code, manager),
        }

        // If filter is active, handle filter-specific keys
        if self.filter_active {
            return self.handle_filter_key(*code, manager);
        }

        // Clear status message on any normal key action
        self.status_message = None;

        match code {
            KeyCode::Char('q') => ScreenAction::Quit,
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.refresh(manager);
                    ScreenAction::None
                } else {
                    ScreenAction::Quit
                }
            }
            KeyCode::Char('/') => {
                self.activate_filter();
                ScreenAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor_down();
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor_up();
                ScreenAction::None
            }
            KeyCode::Enter => self.handle_enter(manager),
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                self.handle_expand(manager);
                ScreenAction::None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.handle_collapse(manager);
                ScreenAction::None
            }
            KeyCode::Char('n') => {
                let screen = super::new_session::NewSessionScreen::new(manager);
                ScreenAction::Push(Screen::NewSession(screen))
            }
            KeyCode::Char('t') => self.action_new_tab(manager),
            KeyCode::Char('r') => self.action_rename(manager),
            KeyCode::Char('R') => self.action_review(manager),
            KeyCode::Char('d') => self.action_delete(manager),
            KeyCode::Char('.') => self.action_action_menu(manager),
            KeyCode::Char('H') => self.action_history(manager),
            KeyCode::Char('c') => self.action_cleanup(manager),
            KeyCode::Char('S') => self.action_settings(manager),
            KeyCode::Char('?') => self.action_help(),
            _ => ScreenAction::None,
        }
    }

    fn on_child_result(&mut self, _result: ActionResult, _manager: &mut Manager) -> ScreenAction {
        ScreenAction::None
    }

    fn on_resume(&mut self, manager: &mut Manager) {
        self.refresh(manager);
    }
}
