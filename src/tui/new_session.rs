// torchard-rs/src/tui/new_session.rs — multi-step wizard for creating a new session

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::git;
use crate::manager::{self, Manager};
use crate::tmux;
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
    PickSubsystem,
}

// ---------------------------------------------------------------------------
// A lightweight repo stand-in (name + path, no DB id required)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SelectedRepo {
    name: String,
    path: String,
}

// ---------------------------------------------------------------------------
// NewSessionScreen
// ---------------------------------------------------------------------------

pub struct NewSessionScreen {
    step: Step,

    // Repo step
    dev_dirs: Vec<(String, String)>, // (name, path)
    filtered_dirs: Vec<(String, String)>,
    repo_list_state: ListState,
    awaiting_repo_path: bool,

    // Branch step
    branches: Vec<String>,
    filtered_branches: Vec<String>,
    branch_list_state: ListState,

    // Subsystem step
    subsystems: Vec<String>,
    filtered_subsystems: Vec<String>,
    subsystem_list_state: ListState,

    // Selections
    selected_repo: Option<SelectedRepo>,
    selected_branch: Option<String>,
    session_name: String,

    // Shared input state
    filter_input: String,
    filter_cursor: usize,

    // Error display
    error: String,
}

impl NewSessionScreen {
    pub fn new(manager: &Manager) -> Self {
        let repos_dir = manager.repos_dir();
        let worktrees_dir = manager.worktrees_dir();

        let mut dev_dirs: Vec<(String, String)> = Vec::new();
        if repos_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&repos_dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().is_dir()
                        && e.path() != worktrees_dir
                        && !e.file_name().to_string_lossy().starts_with('.')
                })
                .collect();
            entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
            for entry in entries {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path().to_string_lossy().to_string();
                dev_dirs.push((name, path));
            }
        }

        let filtered_dirs = dev_dirs.clone();
        let mut repo_list_state = ListState::default();
        if !filtered_dirs.is_empty() {
            repo_list_state.select(Some(0));
        }

        Self {
            step: Step::PickRepo,
            dev_dirs,
            filtered_dirs,
            repo_list_state,
            awaiting_repo_path: false,
            branches: Vec::new(),
            filtered_branches: Vec::new(),
            branch_list_state: ListState::default(),
            subsystems: Vec::new(),
            filtered_subsystems: Vec::new(),
            subsystem_list_state: ListState::default(),
            selected_repo: None,
            selected_branch: None,
            session_name: String::new(),
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
        self.filtered_dirs = self.dev_dirs.clone();
        self.repo_list_state = ListState::default();
        if !self.filtered_dirs.is_empty() {
            self.repo_list_state.select(Some(0));
        }
    }

    fn go_to_branch_step(&mut self, manager: &Manager) {
        self.step = Step::PickBranch;
        self.selected_branch = None;
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
        let _ = manager; // may be used later
    }

    fn advance_after_branch(&mut self, manager: &Manager) -> ScreenAction {
        self.session_name = self.auto_session_name(manager);
        let repo = self.selected_repo.as_ref().unwrap();
        self.subsystems = manager::detect_subsystems(&repo.path);
        if !self.subsystems.is_empty() {
            self.go_to_subsystem_step();
            ScreenAction::None
        } else {
            self.create_session(manager)
        }
    }

    fn go_to_subsystem_step(&mut self) {
        self.step = Step::PickSubsystem;
        self.filter_input.clear();
        self.filter_cursor = 0;
        self.error.clear();
        self.filtered_subsystems = self.subsystems.clone();
        self.subsystem_list_state = ListState::default();
        self.subsystem_list_state.select(Some(0)); // "/ (root)" is always present
    }

    // ------------------------------------------------------------------
    // Auto-naming
    // ------------------------------------------------------------------

    fn auto_session_name(&self, manager: &Manager) -> String {
        let repo = self.selected_repo.as_ref().unwrap();
        let branch = self.selected_branch.as_ref().unwrap();

        let default = git::detect_default_branch(&repo.path).unwrap_or_else(|_| "main".into());
        let base = if branch == &default {
            &repo.name
        } else {
            branch
        };
        let name = tmux::sanitize_session_name(base);
        if manager.get_session_by_name(&name).is_none() {
            return name;
        }
        for i in 2..100 {
            let candidate = format!("{}-{}", name, i);
            if manager.get_session_by_name(&candidate).is_none() {
                return candidate;
            }
        }
        name
    }

    // ------------------------------------------------------------------
    // Session creation
    // ------------------------------------------------------------------

    fn create_session(&mut self, manager: &Manager) -> ScreenAction {
        let repo = self.selected_repo.as_ref().unwrap();
        let branch = self.selected_branch.as_ref().unwrap();
        // Determine subdirectory (None if not in subsystem step or root selected)
        let subdirectory: Option<String> = if self.step == Step::PickSubsystem {
            // The selected subsystem index: 0 = root, 1+ = subsystem
            if let Some(idx) = self.subsystem_list_state.selected() {
                if idx == 0 {
                    None // root
                } else {
                    self.filtered_subsystems.get(idx - 1).cloned()
                }
            } else {
                None
            }
        } else {
            None
        };

        let subdir_ref = subdirectory.as_deref();

        // Try to create
        // manager.create_session may panic on worktree failures, but we'll just call it
        manager.create_session(&repo.path, branch, &self.session_name, subdir_ref);

        let _ = tmux::switch_client(&self.session_name);
        ScreenAction::Quit
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

    fn finish_add_repo(&mut self, manager: &Manager) -> ScreenAction {
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
        self.go_to_branch_step(manager);
        ScreenAction::None
    }

    // ------------------------------------------------------------------
    // Filtering
    // ------------------------------------------------------------------

    fn apply_filter(&mut self) {
        let query = self.filter_input.to_lowercase();
        match self.step {
            Step::PickRepo => {
                self.filtered_dirs = self
                    .dev_dirs
                    .iter()
                    .filter(|(n, p)| n.to_lowercase().contains(&query) || p.to_lowercase().contains(&query))
                    .cloned()
                    .collect();
                self.repo_list_state = ListState::default();
                if !self.filtered_dirs.is_empty() {
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
            Step::PickSubsystem => {
                self.filtered_subsystems = self
                    .subsystems
                    .iter()
                    .filter(|s| s.to_lowercase().contains(&query))
                    .cloned()
                    .collect();
                self.subsystem_list_state = ListState::default();
                // Always have root at index 0
                self.subsystem_list_state.select(Some(0));
            }
        }
    }

    // ------------------------------------------------------------------
    // List navigation
    // ------------------------------------------------------------------

    fn list_len(&self) -> usize {
        match self.step {
            Step::PickRepo => self.filtered_dirs.len() + 1, // +1 for "add repo"
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
            Step::PickSubsystem => self.filtered_subsystems.len() + 1, // +1 for root
        }
    }

    fn selected_index(&self) -> Option<usize> {
        match self.step {
            Step::PickRepo => self.repo_list_state.selected(),
            Step::PickBranch => self.branch_list_state.selected(),
            Step::PickSubsystem => self.subsystem_list_state.selected(),
        }
    }

    fn select_index(&mut self, idx: usize) {
        match self.step {
            Step::PickRepo => self.repo_list_state.select(Some(idx)),
            Step::PickBranch => self.branch_list_state.select(Some(idx)),
            Step::PickSubsystem => self.subsystem_list_state.select(Some(idx)),
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
    // Confirm selection (Enter on list)
    // ------------------------------------------------------------------

    fn confirm_selection(&mut self, manager: &Manager) -> ScreenAction {
        match self.step {
            Step::PickRepo => self.confirm_repo(manager),
            Step::PickBranch => self.confirm_branch(manager),
            Step::PickSubsystem => self.confirm_subsystem(manager),
        }
    }

    fn confirm_repo(&mut self, manager: &Manager) -> ScreenAction {
        let idx = match self.repo_list_state.selected() {
            Some(i) => i,
            None => {
                if !self.filtered_dirs.is_empty() {
                    0
                } else {
                    // No items, maybe "add repo" is the only option
                    self.enter_repo_path_mode();
                    return ScreenAction::None;
                }
            }
        };
        if idx >= self.filtered_dirs.len() {
            // "Add repo" item
            self.enter_repo_path_mode();
            return ScreenAction::None;
        }
        let (name, path) = self.filtered_dirs[idx].clone();
        self.selected_repo = Some(SelectedRepo { name, path });
        self.go_to_branch_step(manager);
        ScreenAction::None
    }

    fn confirm_branch(&mut self, manager: &Manager) -> ScreenAction {
        let idx = self.branch_list_state.selected().unwrap_or(0);
        if idx < self.filtered_branches.len() {
            self.selected_branch = Some(self.filtered_branches[idx].clone());
        } else {
            // "New branch" item — use the typed text
            if self.filter_input.is_empty() {
                return ScreenAction::None;
            }
            self.selected_branch = Some(self.filter_input.clone());
        }
        self.advance_after_branch(manager)
    }

    fn confirm_subsystem(&mut self, manager: &Manager) -> ScreenAction {
        // Index 0 = root, 1+ = subsystem entries
        self.create_session(manager)
    }

    // ------------------------------------------------------------------
    // Rendering helpers
    // ------------------------------------------------------------------

    fn title(&self) -> String {
        match self.step {
            Step::PickRepo => "Step 1 - Select Repository".to_string(),
            Step::PickBranch => {
                let repo_name = self.selected_repo.as_ref().map(|r| r.name.as_str()).unwrap_or("?");
                format!("Step 2 - Select Branch ({})", repo_name)
            }
            Step::PickSubsystem => {
                let repo_name = self.selected_repo.as_ref().map(|r| r.name.as_str()).unwrap_or("?");
                format!("Step 3 - Working Directory ({})", repo_name)
            }
        }
    }

    fn hint(&self) -> &str {
        if self.awaiting_repo_path {
            return "Enter the full path to a git repo, then press Enter.";
        }
        match self.step {
            Step::PickRepo => "Type to filter. Enter to select. Escape to cancel.",
            Step::PickBranch => "Type to filter or enter a new branch name. Enter to confirm.",
            Step::PickSubsystem => "Pick a subsystem to start in, or select root. Enter to confirm.",
        }
    }

    fn placeholder(&self) -> &str {
        if self.awaiting_repo_path {
            return "e.g. /home/you/dev/myproject";
        }
        match self.step {
            Step::PickRepo => "Filter repos...",
            Step::PickBranch => "Filter or type a new branch...",
            Step::PickSubsystem => "Filter...",
        }
    }

    fn build_list_items(&self) -> Vec<ListItem<'_>> {
        match self.step {
            Step::PickRepo => {
                let mut items: Vec<ListItem> = self
                    .filtered_dirs
                    .iter()
                    .map(|(name, path)| {
                        let line = Line::from(vec![
                            Span::styled(name.as_str(), Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD)),
                            Span::styled("  ", Style::default()),
                            Span::styled(path.as_str(), Style::default().fg(theme::TEXT_DIM)),
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
                // Show "+ New branch: <typed>" when filter text doesn't match existing
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
            Step::PickSubsystem => {
                let mut items: Vec<ListItem> = Vec::new();
                items.push(ListItem::new(Span::styled(
                    "/ (root)",
                    Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD),
                )));
                for sub in &self.filtered_subsystems {
                    items.push(ListItem::new(Span::styled(
                        sub.as_str(),
                        Style::default().fg(theme::TEXT),
                    )));
                }
                items
            }
        }
    }
}

impl ScreenBehavior for NewSessionScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let title = self.title();
        let inner = render_modal_box(f, area, &title, 80, 28);

        // Layout: hint (1) + gap (1) + input (1) + gap (1) + list (variable) + error (1)
        let chunks = Layout::vertical([
            Constraint::Length(1), // hint
            Constraint::Length(1), // spacer
            Constraint::Length(1), // filter input
            Constraint::Length(1), // spacer
            Constraint::Min(4),   // list
            Constraint::Length(1), // error
        ])
        .split(inner);

        // Hint
        let hint = Paragraph::new(Span::styled(self.hint(), Style::default().fg(theme::TEXT_DIM)))
            .alignment(Alignment::Center);
        f.render_widget(hint, chunks[0]);

        // Filter input
        if self.filter_input.is_empty() && !self.awaiting_repo_path {
            // Show placeholder
            let ph = Paragraph::new(Span::styled(
                self.placeholder(),
                Style::default().fg(Color::Rgb(0x66, 0x66, 0x66)),
            ));
            f.render_widget(ph, chunks[2]);
            // Still show cursor at position 0
            let cursor_area = Rect { x: chunks[2].x, y: chunks[2].y, width: chunks[2].width, height: 1 };
            render_input(f, cursor_area, "", 0);
        } else {
            render_input(f, chunks[2], &self.filter_input, self.filter_cursor);
        }

        // List (only show when not in path entry mode, or always show for context)
        if !self.awaiting_repo_path {
            let items = self.build_list_items();
            let list_state = match self.step {
                Step::PickRepo => &self.repo_list_state,
                Step::PickBranch => &self.branch_list_state,
                Step::PickSubsystem => &self.subsystem_list_state,
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
                        // Cancel path entry, return to repo list
                        self.awaiting_repo_path = false;
                        self.filter_input.clear();
                        self.filter_cursor = 0;
                        self.error.clear();
                        self.filtered_dirs = self.dev_dirs.clone();
                        self.repo_list_state = ListState::default();
                        if !self.filtered_dirs.is_empty() {
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
                        Step::PickSubsystem => {
                            // Skip back to branch (not name step)
                            self.go_to_branch_step(manager);
                            return ScreenAction::None;
                        }
                    }
                }
                KeyCode::Enter => {
                    if self.awaiting_repo_path {
                        return self.finish_add_repo(manager);
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
                    // Text input for filter
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
