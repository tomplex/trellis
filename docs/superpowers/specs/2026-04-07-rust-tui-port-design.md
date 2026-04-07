# Torchard Rust TUI Port

Port the Torchard TUI from Python/Textual to Rust/ratatui for near-instant startup. The Rust crate coexists alongside the Python package -- nothing is deleted until the new binary is confirmed equivalent.

## Motivation

Startup latency is the sole pain point. Python import overhead + Textual framework init adds 200-500ms before anything renders. The TUI pops up above tmux and needs to feel instant. A compiled Rust binary eliminates this entirely.

Interaction speed is fine today and not a goal of this port.

## Constraints

- **Coexistence:** The Rust crate lives at `torchard-rs/` alongside the existing `torchard/` Python package. Both remain functional. The Python code is not modified or deleted.
- **Same database:** The Rust binary reads and writes the same SQLite database at `~/.local/share/torchard/torchard.db`. Same schema, same migrations. No data migration required.
- **Same switch protocol:** The Rust binary writes the same `/tmp/torchard-switch.json` format and handles the deferred tmux switch after TUI exit, same as `__main__.py`.
- **Same behavior:** Every screen does exactly what its Python counterpart does. No feature additions, no behavior changes, no UX experiments.

## Project Structure

```
torchard/                # existing Python package (untouched)
torchard-rs/
  Cargo.toml
  src/
    main.rs              # entry point: init DB, run TUI, handle switch file
    db.rs                # rusqlite wrapper, same schema/queries
    models.rs            # Repo, Session, Worktree, SessionInfo structs
    manager.rs           # orchestration (mirrors Python Manager)
    tmux.rs              # Command::new("tmux") wrappers
    git.rs               # Command::new("git") / Command::new("gh") wrappers
    claude_session.rs    # pane classification, JSONL reading, summarize
    fuzzy.rs             # fuzzy match scoring
    switch.rs            # write/read switch JSON
    tui/
      mod.rs             # App struct, screen enum, main event loop
      session_list.rs    # main list view
      new_session.rs     # multi-step wizard
      review.rs          # PR/branch checkout
      adopt_session.rs
      rename.rs          # session + window rename
      edit_branch.rs
      new_tab.rs
      action_menu.rs     # reusable picker modal
      confirm.rs         # yes/no modal
      history.rs         # conversation browser
      cleanup.rs         # stale worktree manager
      settings.rs
      help.rs
```

## Dependencies

```toml
[dependencies]
ratatui = "0.29"
crossterm = "0.28"
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "6"
md-5 = "0.10"       # stable repo color hashing
regex = "1"          # pane classification, tmux output parsing
```

No async runtime. Everything is synchronous `Command::new()` calls + blocking `crossterm::event::read()`.

## Core Layer

The core layer is a 1:1 port of the Python core. Each Python module maps to a Rust module with the same functions and same semantics.

### models.rs

Plain structs mirroring the Python dataclasses:

- `Repo { id: Option<i64>, path: String, name: String, default_branch: String }`
- `Session { id: Option<i64>, name: String, repo_id: i64, base_branch: String, created_at: String, last_selected_at: Option<String> }`
- `Worktree { id: Option<i64>, session_id: Option<i64>, repo_id: i64, path: String, branch: String, tmux_window: Option<i64>, created_at: String }`
- `SessionInfo { id: Option<i64>, name: String, repo_id: Option<i64>, base_branch: Option<String>, created_at: Option<String>, last_selected_at: Option<String>, windows: Option<i64>, attached: bool, live: bool, managed: bool }`

### db.rs

Opens the same SQLite file. Runs the same CREATE TABLE IF NOT EXISTS statements. Runs the same `last_selected_at` migration. Exposes the same CRUD functions (`add_repo`, `get_repos`, `add_session`, `get_sessions`, `get_session_by_name`, `touch_session`, `delete_session`, `add_worktree`, `get_worktrees`, `get_worktrees_for_session`, `delete_worktree`, `get_config`, `set_config`, `get_all_config`). All take `&Connection` instead of `sqlite3.Connection`.

### tmux.rs

Same functions as `tmux.py`: `list_sessions`, `session_exists`, `new_session`, `switch_client`, `new_window`, `select_window`, `rename_window`, `rename_session`, `list_all_windows`, `list_windows`, `kill_window`, `send_keys`, `capture_pane`, `get_pane_pid`, `kill_session`, `sanitize_session_name`. Returns `Result<T, TmuxError>` instead of raising exceptions.

### git.rs

Same functions as `git.py`: `detect_default_branch`, `list_branches`, `list_worktrees`, `create_worktree`, `remove_worktree`, `get_pr_branch`, `fetch_and_pull`, `fetch_branch`, `is_branch_merged`, `has_remote_branch`. Returns `Result<T, GitError>`.

### manager.rs

`Manager` struct holds `Connection`. Same public API as the Python `Manager` class:
- `create_session()`, `adopt_session()`, `rename_session()`, `set_base_branch()`
- `checkout_and_review()`, `add_tab()`
- `delete_session()`, `cleanup_worktree()`, `get_stale_worktrees()`
- `scan_existing()` (first-run adoption)
- `list_sessions()` (DB sessions enriched with live tmux state + unmanaged sessions)
- Convenience wrappers: `get_repos()`, `get_sessions()`, `get_worktrees_for_session()`, `touch_session()`, `get_session_by_name()`

The `list_sessions()` enrichment logic (merge DB sessions with `tmux::list_sessions()` output, append unmanaged sessions) is identical to the Python version.

### claude_session.rs

- `get_session_id(pane_pid)` -- reads `/tmp/claude-sessions/pid-{pane_pid}`
- `get_first_user_message(session_id)` -- scans `~/.claude/projects/*/session_id.jsonl` for first user message
- `summarize_message(msg)` -- first line, first 4 words, kebab-case, max 30 chars
- `classify_pane(pane_text)` -- same regex patterns: "prompting" (numbered choices + Esc to cancel), "working" (non-ASCII + word ending in ellipsis), else "idle"

### fuzzy.rs

Same scoring algorithm: characters must appear in order, consecutive matches get -1 bonus, gaps penalized by position. Returns `Option<i32>`.

### switch.rs

```rust
enum SwitchAction {
    Session { target: String },
    Window { session: String, window: i64 },
}
```

`write_switch(action)` serializes to `/tmp/torchard-switch.json`. `read_switch()` deserializes. `cleanup()` removes the file.

## TUI Architecture

### Screen-per-struct with stack navigation

Each screen is its own struct implementing a `ScreenBehavior` trait. A top-level `App` manages a stack of screens, mirroring Textual's `push_screen` / `pop_screen`.

```rust
trait ScreenBehavior {
    fn render(&self, f: &mut Frame, area: Rect, manager: &Manager);
    fn handle_event(&mut self, event: Event, manager: &mut Manager) -> ScreenAction;
    fn on_child_result(&mut self, result: ActionResult, manager: &mut Manager) -> ScreenAction;
    fn on_resume(&mut self, manager: &mut Manager);
}

enum ScreenAction {
    None,
    Push(Screen),
    Pop,
    PopWith(ActionResult),
    Switch(SwitchAction),
    Quit,
}

enum ActionResult {
    Confirmed(bool),              // from ConfirmModal
    MenuPick(Option<String>),     // from ActionMenu (key of selected item, or None if dismissed)
}
```

### App struct and event loop

```rust
struct App {
    manager: Manager,
    screen_stack: Vec<Screen>,
    should_quit: bool,
}
```

Main loop:
1. `terminal.draw(|f| app.render(f))` -- delegates to top screen
2. `crossterm::event::read()` -- blocking, no polling
3. Top screen's `handle_event()` returns a `ScreenAction`
4. App processes the action (push, pop, quit, etc.)
5. On `Pop`, calls `on_resume()` on the new top screen (triggers table refresh, etc.)
6. On `PopWith`, calls `on_child_result()` on the new top screen with the result

### Rendering

Immediate-mode rendering via ratatui. Each screen builds layout with `Layout::vertical` / `Layout::horizontal` and renders widgets. The session list uses ratatui's `Table` widget with styled `Row`/`Cell` items. Textual's rich markup (`[green]...[/green]`) becomes ratatui `Span` with `Style`.

Modal screens (ActionMenu, Confirm) render the parent screen dimmed first, then draw the modal in a centered rect on top.

The footer keybinding bar is a manually rendered `Paragraph` showing available keys for the current screen.

### Color scheme

Same dark theme as the Textual CSS:
- Background: `#1a1a2e`
- Header: `#16213e` with `#00aaff` text
- Cursor row: `#0f3460` with white text
- Footer: `#16213e` with `#aaaaaa` text, keys in `#00aaff` bold
- Repo colors: same 8-color palette, same MD5-hash-mod-8 assignment

## Screen-by-Screen Notes

### SessionListScreen

The most complex screen. State: `sessions: Vec<SessionInfo>`, `repos: HashMap<i64, Repo>`, `expanded: HashSet<String>`, `filter: String`, `cursor: usize`, `filter_active: bool`.

- `refresh()` queries `manager.list_sessions()` + `tmux::list_all_windows()`, sorts/filters, rebuilds the row list
- Sort order: main pinned first, then by `last_selected_at` descending, then alphabetical
- Fuzzy filter matches against session name, repo name, and branch
- Expand/collapse shows child rows for tmux windows with Claude pane state classification
- All keybindings match: q, /, j/k/arrows, enter, tab, n, r, d, h, ., c, S, ?

### NewSessionScreen

Multi-step wizard with an enum tracking the current step:

- `PickRepo` -- filterable list of known repos, option to add by path
- `PickBranch` -- filterable list of branches for the selected repo
- `ConfirmName` -- editable session name, enter to create

Step transitions replace the enum variant. Cleaner than the Python version's show/hide widget approach.

### HistoryScreen

Parses `~/.claude/conversation-index.md` (same line-by-line parsing as `conversation_index.py`). Filterable list scoped to repo paths. Enter resumes a conversation.

### CleanupScreen

Multi-select list of stale worktrees (merged or remote-deleted branches). `HashSet<i64>` tracks selections, space toggles, enter confirms bulk deletion.

### Simple screens

- **ReviewScreen** -- text input for PR number or branch name, enter to checkout
- **AdoptSessionScreen** -- pick repo + branch for an unmanaged tmux session
- **RenameSessionScreen / RenameWindowScreen** -- text input, enter to confirm
- **EditBranchScreen** -- text input, enter to confirm
- **NewTabScreen** -- text input for branch name
- **SettingsScreen** -- edit repos_dir and worktrees_dir
- **HelpScreen** -- static keybinding reference
- **ActionMenu** -- generic picker, returns selection via PopWith
- **ConfirmModal** -- yes/no, returns result via PopWith

## Entry Point

```rust
fn main() {
    let db_path = dirs::data_dir().unwrap().join("torchard/torchard.db");
    let first_run = !db_path.exists();
    let conn = db::init_db(&db_path);
    let mut manager = Manager::new(conn);
    if first_run {
        manager.scan_existing();
    }

    let mut terminal = ratatui::init();
    let mut app = App::new(manager);
    app.run(&mut terminal);
    ratatui::restore();

    if let Some(action) = switch::read_switch() {
        match action {
            SwitchAction::Session { target } => { tmux::switch_client(&target).ok(); }
            SwitchAction::Window { session, window } => {
                tmux::switch_client(&format!("{session}:{window}")).ok();
            }
        }
        switch::cleanup();
    }
}
```

Binary name: `torchard-rs` during coexistence period.

## Testing Strategy

- **Core layer:** Unit tests for db, fuzzy, claude_session, switch modules. Integration tests that create a temp SQLite DB and exercise Manager methods.
- **TUI:** Manual testing against the Python version. Run both side-by-side, confirm identical behavior for each screen and interaction.
- **No mocking of tmux/git:** Tests that need tmux/git state are integration tests run manually, same as the Python test approach.

## Out of Scope

- Async/tokio -- not needed, everything is synchronous CLI calls
- New features or UX changes -- this is a behavior-preserving port
- Deleting the Python version -- happens later, after confirmation
- Config file migration -- not needed, same DB
- Cross-platform support -- macOS only, same as today
