// torchard-rs/src/tui/adopt_session.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::git;
use crate::manager::Manager;
use crate::models::Repo;
use super::rename::{input_handle_key, render_input, render_modal_box};
use super::{ScreenAction, ScreenBehavior};
use super::theme;

// ---------------------------------------------------------------------------
// Wizard step
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    PickRepo,
    PickBranch,
}

// ---------------------------------------------------------------------------
// A repo selection (either from DB or ad-hoc path entry)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SelectedRepo {
    name: String,
    path: String,
}

// ---------------------------------------------------------------------------
// AdoptSessionScreen
// ---------------------------------------------------------------------------

pub struct AdoptSessionScreen {
    session_name: String,
    step: Step,

    // Repo step
    repos: Vec<Repo>,
    filtered_repos: Vec<usize>, // indices into repos
    repo_list_state: ListState,
    awaiting_repo_path: bool,

    // Branch step
    branches: Vec<String>,
    filtered_branches: Vec<String>,
    branch_list_state: ListState,

    // Selection
    selected_repo: Option<SelectedRepo>,

    // Shared filter input
    filter_input: String,
    filter_cursor: usize,

    // Error display
    error: String,
}

impl AdoptSessionScreen {
    pub fn new(manager: &Manager, session_name: String) -> Self {
        let repos = manager.get_repos();
        let filtered_repos: Vec<usize> = (0..repos.len()).collect();
        let mut repo_list_state = ListState::default();
        if !filtered_repos.is_empty() {
            repo_list_state.select(Some(0));
        }
        Self {
            session_name,
            step: Step::PickRepo,
            repos,
            filtered_repos,
            repo_list_state,
            awaiting_repo_path: false,
            branches: Vec::new(),
            filtered_branches: Vec::new(),
            branch_list_state: ListState::default(),
            selected_repo: None,
            filter_input: String::new(),
            filter_cursor: 0,
            error: String::new(),
        }
    }

    // ------------------------------------------------------------------
    // Step transitions
    // ------------------------------------------------------------------

    fn go_to_repo_step(&mut self) {
        self.step = Step::PickRepo;
        self.selected_repo = None;
        self.filter_input.clear();
        self.filter_cursor = 0;
        self.error.clear();
        self.awaiting_repo_path = false;
        self.filtered_repos = (0..self.repos.len()).collect();
        self.repo_list_state = ListState::default();
        if !self.filtered_repos.is_empty() {
            self.repo_list_state.select(Some(0));
        }
    }

    fn go_to_branch_step(&mut self) {
        self.step = Step::PickBranch;
        self.filter_input.clear();
        self.filter_cursor = 0;
        self.error.clear();

        let repo = self.selected_repo.as_ref().unwrap();
        match git::list_branches(&repo.path) {
            Ok(branches) => {
                self.branches = branches;
            }
            Err(e) => {
                self.branches = Vec::new();
                self.error = e.to_string();
            }
        }
        self.filtered_branches = self.branches.clone();
        self.branch_list_state = ListState::default();
        if !self.filtered_branches.is_empty() {
            self.branch_list_state.select(Some(0));
        }
    }

    // ------------------------------------------------------------------
    // Path entry mode
    // ------------------------------------------------------------------

    fn enter_repo_path_mode(&mut self) {
        self.awaiting_repo_path = true;
        self.filter_input.clear();
        self.filter_cursor = 0;
        self.error.clear();
    }

    fn finish_add_repo(&mut self) -> ScreenAction {
        let path_str = self.filter_input.trim().to_string();
        let path = if path_str.starts_with('~') {
            let home = dirs::home_dir().unwrap_or_default();
            home.join(path_str.strip_prefix("~/").unwrap_or(&path_str[1..]))
        } else {
            std::path::PathBuf::from(&path_str)
        };
        let path = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => path,
        };

        if !path.is_dir() || !path.join(".git").exists() {
            self.error = format!("'{}' is not a git repository.", path.display());
            self.awaiting_repo_path = false;
            self.filter_input.clear();
            self.filter_cursor = 0;
            return ScreenAction::None;
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let path_s = path.to_string_lossy().to_string();
        self.selected_repo = Some(SelectedRepo { name, path: path_s });
        self.awaiting_repo_path = false;
        self.go_to_branch_step();
        ScreenAction::None
    }

    // ------------------------------------------------------------------
    // Adopt
    // ------------------------------------------------------------------

    fn adopt(&mut self, base_branch: &str, manager: &mut Manager) -> ScreenAction {
        let repo = self.selected_repo.as_ref().unwrap();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            manager.adopt_session(&self.session_name, &repo.path, base_branch)
        })) {
            Ok(_) => ScreenAction::Pop,
            Err(_) => {
                self.error = "Failed to adopt session.".to_string();
                ScreenAction::None
            }
        }
    }

    // ------------------------------------------------------------------
    // Filtering
    // ------------------------------------------------------------------

    fn apply_filter(&mut self) {
        let query = self.filter_input.to_lowercase();
        match self.step {
            Step::PickRepo => {
                self.filtered_repos = self
                    .repos
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        r.name.to_lowercase().contains(&query)
                            || r.path.to_lowercase().contains(&query)
                    })
                    .map(|(i, _)| i)
                    .collect();
                self.repo_list_state = ListState::default();
                if !self.filtered_repos.is_empty() {
                    self.repo_list_state.select(Some(0));
                }
            }
            Step::PickBranch => {
                self.filtered_branches = self
                    .branches
                    .iter()
                    .filter(|b| b.to_lowercase().contains(&query))
                    .cloned()
                    .collect();
                self.branch_list_state = ListState::default();
                if !self.filtered_branches.is_empty() {
                    self.branch_list_state.select(Some(0));
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // List navigation
    // ------------------------------------------------------------------

    fn list_len(&self) -> usize {
        match self.step {
            Step::PickRepo => self.filtered_repos.len() + 1, // +1 for "add repo"
            Step::PickBranch => {
                let extra = if !self.filter_input.is_empty()
                    && !self.filtered_branches.contains(&self.filter_input)
                {
                    1
                } else {
                    0
                };
                self.filtered_branches.len() + extra
            }
        }
    }

    fn selected_index(&self) -> Option<usize> {
        match self.step {
            Step::PickRepo => self.repo_list_state.selected(),
            Step::PickBranch => self.branch_list_state.selected(),
        }
    }

    fn select_index(&mut self, idx: usize) {
        match self.step {
            Step::PickRepo => self.repo_list_state.select(Some(idx)),
            Step::PickBranch => self.branch_list_state.select(Some(idx)),
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let current = self.selected_index().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(len as i32) as usize;
        self.select_index(next);
    }

    // ------------------------------------------------------------------
    // Confirm selection (Enter)
    // ------------------------------------------------------------------

    fn confirm_selection(&mut self, manager: &mut Manager) -> ScreenAction {
        match self.step {
            Step::PickRepo => self.confirm_repo(manager),
            Step::PickBranch => self.confirm_branch(manager),
        }
    }

    fn confirm_repo(&mut self, _manager: &mut Manager) -> ScreenAction {
        let idx = match self.repo_list_state.selected() {
            Some(i) => i,
            None => {
                if self.filtered_repos.is_empty() {
                    self.enter_repo_path_mode();
                    return ScreenAction::None;
                }
                0
            }
        };
        if idx >= self.filtered_repos.len() {
            // "Add new repo path..." item
            self.enter_repo_path_mode();
            return ScreenAction::None;
        }
        let repo_idx = self.filtered_repos[idx];
        let repo = &self.repos[repo_idx];
        self.selected_repo = Some(SelectedRepo {
            name: repo.name.clone(),
            path: repo.path.clone(),
        });
        self.go_to_branch_step();
        ScreenAction::None
    }

    fn confirm_branch(&mut self, manager: &mut Manager) -> ScreenAction {
        let idx = self.branch_list_state.selected().unwrap_or(0);
        let branch = if idx < self.filtered_branches.len() {
            self.filtered_branches[idx].clone()
        } else {
            // "New branch" item — use the typed text
            if self.filter_input.is_empty() {
                return ScreenAction::None;
            }
            self.filter_input.clone()
        };
        self.adopt(&branch, manager)
    }

    // ------------------------------------------------------------------
    // Rendering helpers
    // ------------------------------------------------------------------

    fn title(&self) -> String {
        match self.step {
            Step::PickRepo => format!("Adopt '{}' — Select Repository", self.session_name),
            Step::PickBranch => {
                let repo_name = self.selected_repo.as_ref().map(|r| r.name.as_str()).unwrap_or("?");
                format!("Adopt '{}' — Select Branch ({})", self.session_name, repo_name)
            }
        }
    }

    fn hint(&self) -> &str {
        if self.awaiting_repo_path {
            return "Enter the full path to a git repo, then press Enter.";
        }
        match self.step {
            Step::PickRepo => "Type to filter. Enter to select. Escape to cancel.",
            Step::PickBranch => "Pick the branch new worktrees will branch from.",
        }
    }

    fn placeholder(&self) -> &str {
        if self.awaiting_repo_path {
            return "e.g. /home/you/dev/myproject";
        }
        match self.step {
            Step::PickRepo => "Filter repos...",
            Step::PickBranch => "Filter or type a new branch...",
        }
    }

    fn build_list_items(&self) -> Vec<ListItem<'_>> {
        match self.step {
            Step::PickRepo => {
                let mut items: Vec<ListItem> = self
                    .filtered_repos
                    .iter()
                    .map(|&i| {
                        let repo = &self.repos[i];
                        let line = Line::from(vec![
                            Span::styled(
                                repo.name.as_str(),
                                Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled("  ", Style::default()),
                            Span::styled(repo.path.as_str(), Style::default().fg(theme::TEXT_DIM)),
                        ]);
                        ListItem::new(line)
                    })
                    .collect();
                items.push(ListItem::new(Span::styled(
                    "+ Add new repo path...",
                    Style::default().fg(theme::GREEN),
                )));
                items
            }
            Step::PickBranch => {
                let mut items: Vec<ListItem> = self
                    .filtered_branches
                    .iter()
                    .map(|b| ListItem::new(Span::styled(b.as_str(), Style::default().fg(theme::TEXT))))
                    .collect();
                if !self.filter_input.is_empty()
                    && !self.filtered_branches.contains(&self.filter_input)
                {
                    let line = Line::from(vec![
                        Span::styled("+ New branch: ", Style::default().fg(theme::GREEN)),
                        Span::styled(
                            self.filter_input.as_str(),
                            Style::default().fg(theme::GREEN).add_modifier(Modifier::BOLD),
                        ),
                    ]);
                    items.push(ListItem::new(line));
                }
                items
            }
        }
    }
}

impl ScreenBehavior for AdoptSessionScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let title = self.title();
        let inner = render_modal_box(f, area, &title, 80, 28);

        let chunks = Layout::vertical([
            Constraint::Length(1), // hint
            Constraint::Length(1), // spacer
            Constraint::Length(1), // filter input
            Constraint::Length(1), // spacer
            Constraint::Min(4),    // list
            Constraint::Length(1), // error
        ])
        .split(inner);

        // Hint
        let hint = Paragraph::new(Span::styled(self.hint(), Style::default().fg(theme::TEXT_DIM)))
            .alignment(Alignment::Center);
        f.render_widget(hint, chunks[0]);

        // Filter input
        if self.filter_input.is_empty() && !self.awaiting_repo_path {
            let ph = Paragraph::new(Span::styled(
                self.placeholder(),
                Style::default().fg(Color::Rgb(0x66, 0x66, 0x66)),
            ));
            f.render_widget(ph, chunks[2]);
            let cursor_area = Rect { x: chunks[2].x, y: chunks[2].y, width: chunks[2].width, height: 1 };
            render_input(f, cursor_area, "", 0);
        } else {
            render_input(f, chunks[2], &self.filter_input, self.filter_cursor);
        }

        // List (only when not in path entry mode)
        if !self.awaiting_repo_path {
            let items = self.build_list_items();
            let list_state = match self.step {
                Step::PickRepo => &self.repo_list_state,
                Step::PickBranch => &self.branch_list_state,
            };

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme::ACCENT))
                        .style(Style::default().bg(theme::BG)),
                )
                .highlight_style(
                    Style::default()
                        .bg(theme::CURSOR_BG)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ");

            let mut state = list_state.clone();
            f.render_stateful_widget(list, chunks[4], &mut state);
        }

        // Error
        if !self.error.is_empty() {
            let err = Paragraph::new(Span::styled(&self.error[..], theme::style_error()))
                .alignment(Alignment::Center);
            f.render_widget(err, chunks[5]);
        }
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) = event {
            match code {
                KeyCode::Esc => {
                    if self.awaiting_repo_path {
                        self.awaiting_repo_path = false;
                        self.filter_input.clear();
                        self.filter_cursor = 0;
                        self.error.clear();
                        self.filtered_repos = (0..self.repos.len()).collect();
                        self.repo_list_state = ListState::default();
                        if !self.filtered_repos.is_empty() {
                            self.repo_list_state.select(Some(0));
                        }
                        return ScreenAction::None;
                    }
                    match self.step {
                        Step::PickRepo => return ScreenAction::Pop,
                        Step::PickBranch => {
                            self.go_to_repo_step();
                            return ScreenAction::None;
                        }
                    }
                }
                KeyCode::Enter => {
                    if self.awaiting_repo_path {
                        return self.finish_add_repo();
                    }
                    return self.confirm_selection(manager);
                }
                KeyCode::Up => {
                    if !self.awaiting_repo_path {
                        self.move_selection(-1);
                    }
                    return ScreenAction::None;
                }
                KeyCode::Down => {
                    if !self.awaiting_repo_path {
                        self.move_selection(1);
                    }
                    return ScreenAction::None;
                }
                KeyCode::Tab => {
                    if !self.awaiting_repo_path {
                        self.move_selection(1);
                    }
                    return ScreenAction::None;
                }
                KeyCode::BackTab => {
                    if !self.awaiting_repo_path {
                        self.move_selection(-1);
                    }
                    return ScreenAction::None;
                }
                _ => {
                    input_handle_key(&mut self.filter_input, &mut self.filter_cursor, *code, *modifiers);
                    if !self.awaiting_repo_path {
                        self.apply_filter();
                    }
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
