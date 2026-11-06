// trellis/src/main.rs
mod models;
mod db;
mod tmux;
mod git;
mod manager;
mod agent_pane;
mod fuzzy;
mod switch;
mod utils;
mod notify;
mod tui;

fn main() {
    // DB path: ~/.local/share/trellis/trellis.db
    let db_path = dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/trellis/trellis.db");
    let first_run = !db_path.exists();
    let conn = db::init_db(&db_path);
    // Hook mode: no TUI, and no switch-file cleanup (a popup may be mid-switch).
    if std::env::args().nth(1).as_deref() == Some("notify") {
        notify::run(&conn);
        return;
    }
    switch::cleanup();
    let mgr = manager::Manager::new(conn);

    let mut terminal = ratatui::init();
    let mut app = tui::App::new(mgr, first_run);
    app.run(&mut terminal);
    ratatui::restore();

    if let Some(action) = switch::read_switch() {
        let target = match &action {
            switch::SwitchAction::Session { target } => target.clone(),
            switch::SwitchAction::Window { session, window } => {
                format!("{}:{}", session, window)
            }
        };
        if tmux::inside_tmux() {
            tmux::switch_client(&target).ok();
        } else {
            tmux::attach_session(&target).ok();
        }
        switch::cleanup();
    }
}
