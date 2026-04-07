// torchard-rs/src/main.rs
mod models;
mod db;
mod tmux;
mod git;
mod manager;
mod claude_session;
mod conversation_index;
mod fuzzy;
mod switch;
mod utils;
mod tui;

fn main() {
    switch::cleanup();

    // Match Python's _DEFAULT_DB_PATH: ~/.local/share/torchard/torchard.db
    let db_path = dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/torchard/torchard.db");
    let first_run = !db_path.exists();
    let conn = db::init_db(&db_path);
    #[allow(unused_mut)]
    let mut mgr = manager::Manager::new(conn);
    if first_run {
        mgr.scan_existing();
    }

    let mut terminal = ratatui::init();
    let mut app = tui::App::new(mgr);
    app.run(&mut terminal);
    ratatui::restore();

    if let Some(action) = switch::read_switch() {
        match &action {
            switch::SwitchAction::Session { target } => {
                tmux::switch_client(target).ok();
            }
            switch::SwitchAction::Window { session, window } => {
                tmux::switch_client(&format!("{}:{}", session, window)).ok();
            }
        }
        switch::cleanup();
    }
}
