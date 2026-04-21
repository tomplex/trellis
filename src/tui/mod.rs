// trellis/src/tui/mod.rs

pub mod theme;
pub mod session_list;
pub mod confirm;
pub mod help;
pub mod new_session;
pub mod adopt_session;
pub mod rename;
pub mod edit_branch;
pub mod history;
pub mod cleanup;
pub mod settings;

/// A lightweight repo stand-in (name + path, no DB id required).
/// Used by wizard screens that let the user pick a repo.
#[derive(Clone)]
pub struct SelectedRepo {
    pub name: String,
    pub path: String,
}

// ---------------------------------------------------------------------------
// Shared wizard helpers
// ---------------------------------------------------------------------------

/// Validate a user-entered path as a git repo and return a SelectedRepo.
pub fn wizard_validate_repo_path(input: &str) -> Result<SelectedRepo, String> {
    let path_str = input.trim().to_string();
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
        return Err(format!("'{}' is not a git repository.", path.display()));
    }

    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let path_s = path.to_string_lossy().to_string();
    Ok(SelectedRepo { name, path: path_s })
}

/// Load branches for a repo path into the provided state, resetting filter and selection.
pub fn wizard_load_branches(
    repo_path: &str,
    branches: &mut Vec<String>,
    filtered_branches: &mut Vec<String>,
    branch_list_state: &mut ratatui::widgets::ListState,
    filter_input: &mut String,
    filter_cursor: &mut usize,
    error: &mut String,
) {
    filter_input.clear();
    *filter_cursor = 0;
    error.clear();

    match crate::git::list_branches(repo_path) {
        Ok(b) => *branches = b,
        Err(e) => {
            *branches = Vec::new();
            *error = e.to_string();
        }
    }
    *filtered_branches = branches.clone();
    *branch_list_state = ratatui::widgets::ListState::default();
    if !filtered_branches.is_empty() {
        branch_list_state.select(Some(0));
    }
}

use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Clear};

use crate::manager::Manager;
use crate::switch::{self, SwitchAction};

#[allow(dead_code)]
pub enum ActionResult {
    Confirmed(bool),
    MenuPick(Option<String>),
}

pub enum ScreenAction {
    None,
    Push(Box<dyn ScreenBehavior>),
    Pop,
    PopWith(ActionResult),
    Switch(SwitchAction),
    Quit,
}

pub trait ScreenBehavior {
    fn render(&self, f: &mut Frame, area: Rect, manager: &Manager);
    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction;
    fn tick(&mut self, _manager: &mut Manager) -> ScreenAction { ScreenAction::None }
    fn on_child_result(&mut self, _result: ActionResult, _manager: &mut Manager) -> ScreenAction {
        ScreenAction::None
    }
    fn on_resume(&mut self, _manager: &mut Manager) {}
    fn is_modal(&self) -> bool {
        false
    }
}

pub struct App {
    pub manager: Manager,
    screen_stack: Vec<Box<dyn ScreenBehavior>>,
    should_quit: bool,
}

impl App {
    pub fn new(manager: Manager) -> Self {
        Self {
            manager,
            screen_stack: Vec::new(),
            should_quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) {
        // Enable mouse support
        crossterm::execute!(
            std::io::stdout(),
            crossterm::event::EnableMouseCapture
        ).ok();

        // Push initial screen
        let initial = session_list::SessionListScreen::new(&self.manager);
        self.screen_stack.push(Box::new(initial));

        while !self.should_quit {
            // Draw
            terminal
                .draw(|f| self.render(f))
                .expect("draw");

            // Tick the top screen (for background work like staleness checks)
            if let Some(top) = self.screen_stack.last_mut() {
                let action = top.tick(&mut self.manager);
                self.process_action(action);
            }

            // Poll for input
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if let Some(top) = self.screen_stack.last_mut() {
                        let action = top.handle_event(&ev, &mut self.manager);
                        self.process_action(action);
                    }
                }
            }
        }

        // Disable mouse support
        crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture
        ).ok();
    }

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        // Render background
        f.render_widget(Block::default().style(theme::style_default()), area);

        // Render all screens in stack
        for (i, screen) in self.screen_stack.iter().enumerate() {
            let is_top = i == self.screen_stack.len() - 1;
            if is_top {
                screen.render(f, area, &self.manager);
            } else if i + 1 < self.screen_stack.len() && self.screen_stack[i + 1].is_modal() {
                // Render parent of a modal, then clear and overlay a dark background.
                screen.render(f, area, &self.manager);
                f.render_widget(Clear, area);
                let dim = Block::default().style(Style::default().bg(Color::Rgb(0x0d, 0x0d, 0x1a)));
                f.render_widget(dim, area);
            }
        }
    }

    fn process_action(&mut self, action: ScreenAction) {
        match action {
            ScreenAction::None => {}
            ScreenAction::Push(screen) => {
                self.screen_stack.push(screen);
            }
            ScreenAction::Pop => {
                self.screen_stack.pop();
                if self.screen_stack.is_empty() {
                    self.should_quit = true;
                } else if let Some(top) = self.screen_stack.last_mut() {
                    top.on_resume(&mut self.manager);
                }
            }
            ScreenAction::PopWith(result) => {
                self.screen_stack.pop();
                if let Some(top) = self.screen_stack.last_mut() {
                    let action = top.on_child_result(result, &mut self.manager);
                    self.process_action(action);
                }
            }
            ScreenAction::Switch(switch_action) => {
                switch::write_switch(&switch_action);
                self.should_quit = true;
            }
            ScreenAction::Quit => {
                self.should_quit = true;
            }
        }
    }
}
