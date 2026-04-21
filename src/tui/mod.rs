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
    Push(Screen),
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

pub enum Screen {
    SessionList(session_list::SessionListScreen),
    Confirm(confirm::ConfirmScreen),
    Help(help::HelpScreen),
    NewSession(new_session::NewSessionScreen),
    AdoptSession(adopt_session::AdoptSessionScreen),
    RenameSession(rename::RenameSessionScreen),
    RenameWindow(rename::RenameWindowScreen),
    EditBranch(edit_branch::EditBranchScreen),
    History(history::HistoryScreen),
    Cleanup(cleanup::CleanupScreen),
    Settings(settings::SettingsScreen),
}

impl Screen {
    fn behavior(&self) -> &dyn ScreenBehavior {
        match self {
            Screen::SessionList(s) => s,
            Screen::Confirm(s) => s,
            Screen::Help(s) => s,
            Screen::NewSession(s) => s,
            Screen::AdoptSession(s) => s,
            Screen::RenameSession(s) => s,
            Screen::RenameWindow(s) => s,
            Screen::EditBranch(s) => s,
            Screen::History(s) => s,
            Screen::Cleanup(s) => s,
            Screen::Settings(s) => s,
        }
    }

    fn behavior_mut(&mut self) -> &mut dyn ScreenBehavior {
        match self {
            Screen::SessionList(s) => s,
            Screen::Confirm(s) => s,
            Screen::Help(s) => s,
            Screen::NewSession(s) => s,
            Screen::AdoptSession(s) => s,
            Screen::RenameSession(s) => s,
            Screen::RenameWindow(s) => s,
            Screen::EditBranch(s) => s,
            Screen::History(s) => s,
            Screen::Cleanup(s) => s,
            Screen::Settings(s) => s,
        }
    }
}

pub struct App {
    pub manager: Manager,
    screen_stack: Vec<Screen>,
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
        self.screen_stack.push(Screen::SessionList(initial));

        while !self.should_quit {
            // Draw
            terminal
                .draw(|f| self.render(f))
                .expect("draw");

            // Tick the top screen (for background work like staleness checks)
            if let Some(top) = self.screen_stack.last_mut() {
                let action = top.behavior_mut().tick(&mut self.manager);
                self.process_action(action);
            }

            // Poll for input
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if let Some(top) = self.screen_stack.last_mut() {
                        let action = top.behavior_mut().handle_event(&ev, &mut self.manager);
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
                screen.behavior().render(f, area, &self.manager);
            } else if i + 1 < self.screen_stack.len() && self.screen_stack[i + 1].behavior().is_modal() {
                // Render parent of a modal, then clear and overlay a dark background.
                screen.behavior().render(f, area, &self.manager);
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
                    top.behavior_mut().on_resume(&mut self.manager);
                }
            }
            ScreenAction::PopWith(result) => {
                self.screen_stack.pop();
                if let Some(top) = self.screen_stack.last_mut() {
                    let action = top.behavior_mut().on_child_result(result, &mut self.manager);
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
