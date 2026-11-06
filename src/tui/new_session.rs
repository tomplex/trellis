// trellis/src/tui/new_session.rs — multi-step wizard for creating a new session

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::git;
use crate::manager::{self, Manager};
use crate::tmux;
use super::path_complete::PathCompleter;
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
    PickAgent,
    PickSubsystem,
}

// ---------------------------------------------------------------------------
use super::SelectedRepo;

/// Agents offered in the wizard: the configured default plus the built-in
/// choices, default first only if it isn't already one of them.
fn agent_choices(default: &str) -> Vec<String> {
    let mut agents: Vec<String> = ["claude", "pi"].iter().map(|a| a.to_string()).collect();
    if !agents.iter().any(|a| a == default) {
        agents.insert(0, default.to_string());
    }
    agents
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
    path_completer: PathCompleter,

    // Branch step
    branches: Vec<String>,
    filtered_branches: Vec<String>,
    branch_list_state: ListState,

    // Agent step
    agents: Vec<String>,
    agent_list_state: ListState,

    // Subsystem step
    subsystems: Vec<String>,
    filtered_subsystems: Vec<String>,
    subsystem_list_state: ListState,

    // Selections
    selected_repo: Option<SelectedRepo>,
    selected_branch: Option<String>,
    selected_agent: Option<String>,
    session_name: String,

    // Shared input state
    filter_input: String,
    filter_cursor: usize,

    // Error display
    error: String,
}

impl NewSessionScreen {
    pub fn new(manager: &Manager) -> Self {
        let repos_dirs = manager.repos_dirs();
        let worktrees_dir = manager.worktrees_dir();

        // Collect (name, abs_path, parent_dir_label) across every configured dir.
        let mut raw: Vec<(String, String, String)> = Vec::new();
        for repos_dir in &repos_dirs {
            if !repos_dir.is_dir() {
                continue;
            }
            let parent_label = repos_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            for path in git::find_repos(repos_dir, &worktrees_dir, git::REPO_SCAN_DEPTH) {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                raw.push((name, path.to_string_lossy().to_string(), parent_label.clone()));
            }
        }

        // Count name occurrences for collision detection.
        let mut name_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (name, _, _) in &raw {
            *name_counts.entry(name.clone()).or_insert(0) += 1;
        }

        // Build display labels: plain name unless that name collides, in
        // which case annotate with the parent dir.
        let mut dev_dirs: Vec<(String, String)> = raw
            .into_iter()
            .map(|(name, path, parent)| {
                let label = if name_counts.get(&name).copied().unwrap_or(0) > 1 {
                    format!("{} ({})", name, parent)
                } else {
                    name
                };
                (label, path)
            })
            .collect();
        dev_dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

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
            path_completer: PathCompleter::default(),
            branches: Vec::new(),
            filtered_branches: Vec::new(),
            branch_list_state: ListState::default(),
            agents: Vec::new(),
            agent_list_state: ListState::default(),
            subsystems: Vec::new(),
            filtered_subsystems: Vec::new(),
            subsystem_list_state: ListState::default(),
            selected_repo: None,
            selected_branch: None,
            selected_agent: None,
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

    fn go_to_branch_step(&mut self) {
        self.step = Step::PickBranch;
        self.selected_branch = None;
        let repo = self.selected_repo.as_ref().unwrap();
        super::wizard_load_branches(
            &repo.path,
            &mut self.branches,
            &mut self.filtered_branches,
            &mut self.branch_list_state,
            &mut self.filter_input,
            &mut self.filter_cursor,
            &mut self.error,
        );
    }

    fn advance_after_branch(&mut self, manager: &Manager) -> ScreenAction {
        self.session_name = self.auto_session_name(manager);
        let default_agent = manager.agent_cmd();
        self.agents = agent_choices(&default_agent);
        self.agent_list_state = ListState::default();
        self.agent_list_state
            .select(self.agents.iter().position(|a| *a == default_agent));
        self.go_to_agent_step();
        ScreenAction::None
    }

    fn go_to_agent_step(&mut self) {
        self.step = Step::PickAgent;
        self.filter_input.clear();
        self.filter_cursor = 0;
        self.error.clear();
    }

    fn advance_after_agent(&mut self, manager: &Manager) -> ScreenAction {
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

        match manager.create_session(&repo.path, branch, &self.session_name, subdir_ref, self.selected_agent.as_deref()) {
            Ok((_, warning)) => {
                let _ = tmux::switch_client(&self.session_name);
                if let Some(w) = warning {
                    tmux::display_message(&self.session_name, &format!("trellis: {w}"));
                }
                ScreenAction::Quit
            }
            Err(e) => {
                self.error = e;
                ScreenAction::None
            }
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

    fn finish_add_repo(&mut self, _manager: &Manager) -> ScreenAction {
        match super::wizard_validate_repo_path(&self.filter_input) {
            Ok(repo) => {
                self.selected_repo = Some(repo);
                self.awaiting_repo_path = false;
                self.go_to_branch_step();
            }
            Err(e) => {
                self.error = e;
                self.awaiting_repo_path = false;
                self.filter_input.clear();
                self.filter_cursor = 0;
            }
        }
        ScreenAction::None
    }

    // ------------------------------------------------------------------
    // Filtering
    // ------------------------------------------------------------------

    fn apply_filter(&mut self) {
        let query = self.filter_input.clone();
        match self.step {
            Step::PickRepo => {
                let mut scored: Vec<((String, String), i32)> = self
                    .dev_dirs
                    .iter()
                    .filter_map(|(n, p)| {
                        let best = [n.as_str(), p.as_str()]
                            .iter()
                            .filter_map(|c| crate::fuzzy::fuzzy_match_multi(&query, c))
                            .min();
                        best.map(|score| ((n.clone(), p.clone()), score))
                    })
                    .collect();
                scored.sort_by(|a, b| {
                    a.1.cmp(&b.1).then_with(|| a.0 .0.to_lowercase().cmp(&b.0 .0.to_lowercase()))
                });
                self.filtered_dirs = scored.into_iter().map(|((n, p), _)| (n, p)).collect();
                self.repo_list_state = ListState::default();
                if !self.filtered_dirs.is_empty() {
                    self.repo_list_state.select(Some(0));
                }
            }
            Step::PickBranch => {
                let mut scored: Vec<(String, i32)> = self
                    .branches
                    .iter()
                    .filter_map(|b| crate::fuzzy::fuzzy_match_multi(&query, b).map(|s| (b.clone(), s)))
                    .collect();
                scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
                self.filtered_branches = scored.into_iter().map(|(b, _)| b).collect();
                self.branch_list_state = ListState::default();
                if !self.filtered_branches.is_empty() {
                    self.branch_list_state.select(Some(0));
                }
            }
            Step::PickAgent => {}
            Step::PickSubsystem => {
                let mut scored: Vec<(String, i32)> = self
                    .subsystems
                    .iter()
                    .filter_map(|s| crate::fuzzy::fuzzy_match_multi(&query, s).map(|sc| (s.clone(), sc)))
                    .collect();
                scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
                self.filtered_subsystems = scored.into_iter().map(|(s, _)| s).collect();
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
            Step::PickAgent => self.agents.len(),
            Step::PickSubsystem => self.filtered_subsystems.len() + 1, // +1 for root
        }
    }

    fn selected_index(&self) -> Option<usize> {
        match self.step {
            Step::PickRepo => self.repo_list_state.selected(),
            Step::PickBranch => self.branch_list_state.selected(),
            Step::PickAgent => self.agent_list_state.selected(),
            Step::PickSubsystem => self.subsystem_list_state.selected(),
        }
    }

    fn select_index(&mut self, idx: usize) {
        match self.step {
            Step::PickRepo => self.repo_list_state.select(Some(idx)),
            Step::PickBranch => self.branch_list_state.select(Some(idx)),
            Step::PickAgent => self.agent_list_state.select(Some(idx)),
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
            Step::PickAgent => self.confirm_agent(manager),
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
        self.go_to_branch_step();
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

    fn confirm_agent(&mut self, manager: &Manager) -> ScreenAction {
        let idx = self.agent_list_state.selected().unwrap_or(0);
        self.selected_agent = self.agents.get(idx).cloned();
        self.advance_after_agent(manager)
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
            Step::PickAgent => {
                let repo_name = self.selected_repo.as_ref().map(|r| r.name.as_str()).unwrap_or("?");
                format!("Step 3 - Agent ({})", repo_name)
            }
            Step::PickSubsystem => {
                let repo_name = self.selected_repo.as_ref().map(|r| r.name.as_str()).unwrap_or("?");
                format!("Step 4 - Working Directory ({})", repo_name)
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
            Step::PickAgent => "Pick the agent to launch. Enter to confirm. Escape to go back.",
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
            Step::PickAgent => "",
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
            Step::PickAgent => self
                .agents
                .iter()
                .map(|a| ListItem::new(Span::styled(a.as_str(), Style::default().fg(theme::TEXT))))
                .collect(),
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
                Step::PickAgent => &self.agent_list_state,
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

        if self.awaiting_repo_path {
            self.path_completer.render(f, chunks[4]);
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
            if !matches!(code, KeyCode::Tab | KeyCode::BackTab) {
                self.path_completer.reset();
            }
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
                        Step::PickAgent => {
                            self.go_to_branch_step();
                            return ScreenAction::None;
                        }
                        Step::PickSubsystem => {
                            self.go_to_agent_step();
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
                    if self.awaiting_repo_path {
                        self.path_completer.complete(&mut self.filter_input, &mut self.filter_cursor, false);
                    } else {
                        self.move_selection(1);
                    }
                    return ScreenAction::None;
                }
                KeyCode::BackTab => {
                    if self.awaiting_repo_path {
                        self.path_completer.complete(&mut self.filter_input, &mut self.filter_cursor, true);
                    } else {
                        self.move_selection(-1);
                    }
                    return ScreenAction::None;
                }
                KeyCode::Char(c @ ('j' | 'n' | 'k' | 'p')) if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    // Vim/Emacs-style nav while filter input has focus. Other Ctrl
                    // chords (Ctrl+U, Ctrl+W) fall through to the input handler.
                    if !self.awaiting_repo_path {
                        match c {
                            'j' | 'n' => self.move_selection(1),
                            'k' | 'p' => self.move_selection(-1),
                            _ => {}
                        }
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

#[cfg(test)]
mod tests {
    use super::agent_choices;
    use crate::fuzzy::fuzzy_match_multi;

    #[test]
    fn agent_choices_always_include_the_default_exactly_once() {
        for default in ["claude", "pi", "omp"] {
            let agents = agent_choices(default);
            assert_eq!(agents.iter().filter(|a| *a == default).count(), 1);
            assert!(agents.contains(&"claude".to_string()) && agents.contains(&"pi".to_string()));
        }
    }

    /// Helper that mirrors what the picker's filter does post-change.
    fn rank(query: &str, items: &[&str]) -> Vec<String> {
        let mut scored: Vec<(String, i32)> = items
            .iter()
            .filter_map(|s| fuzzy_match_multi(query, s).map(|score| (s.to_string(), score)))
            .collect();
        scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        scored.into_iter().map(|(s, _)| s).collect()
    }

    #[test]
    fn filter_ranks_subsequence_above_substring_only() {
        let items = ["feat", "feature", "unrelated"];
        let ranked = rank("ft", &items);
        assert_eq!(ranked, vec!["feat".to_string(), "feature".to_string()]);
    }

    #[test]
    fn filter_alphabetic_tiebreak_when_scores_equal() {
        // Both candidates have the query char 'a' at position 0 -> identical
        // score (0) from fuzzy_match -> alphabetical tiebreak picks "aaa" first.
        let items = ["aaa", "abc"];
        let ranked = rank("a", &items);
        assert_eq!(ranked, vec!["aaa".to_string(), "abc".to_string()]);
    }
}
