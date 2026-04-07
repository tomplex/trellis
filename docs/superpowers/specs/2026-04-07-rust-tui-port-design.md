# Torchard Rust TUI Port

Port the Torchard TUI from Python/Textual to Rust/ratatui for near-instant startup. The Rust crate coexists alongside the Python package -- nothing is deleted until the new binary is confirmed equivalent.

## Motivation

Startup latency is the sole pain point. Python import overhead + Textual framework init adds 200-500ms before anything renders. The TUI pops up above tmux and needs to feel instant. A compiled Rust binary eliminates this entirely.

Interaction speed is fine today and not a goal of this port.

## Constraints

- **Coexistence:** The Rust crate lives at `torchard-rs/` alongside the existing `torchard/` Python package. Both remain functional. The Python code is not modified or deleted.
- **Same database:** The Rust binary reads and writes the same SQLite database at `~/.local/share/torchard/torchard.db`. Same schema, same migrations. No data migration required.
- **Same switch protocol:** The Rust binary writes the same switch JSON file to `std::env::temp_dir()` (matching Python's `tempfile.gettempdir()`, which on macOS is a user-specific path like `/var/folders/.../T/`, not `/tmp/`). Handles the deferred tmux switch after TUI exit, same as `__main__.py`.
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
    conversation_index.rs # parse conversation-index.md, Conversation struct
    fuzzy.rs             # fuzzy match scoring
    switch.rs            # write/read switch JSON
    utils.rs             # truncate_end, truncate_start helpers
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

No async runtime. The main event loop uses `crossterm::event::poll()` with a short timeout (e.g. 50ms) rather than blocking `read()`, so it can check for results from background threads (see Background Work section).

## Core Layer

The core layer is a 1:1 port of the Python core. Each Python module maps to a Rust module with the same functions and same semantics.

### models.rs

Plain structs mirroring the Python dataclasses:

- `Repo { id: Option<i64>, path: String, name: String, default_branch: String }`
- `Session { id: Option<i64>, name: String, repo_id: i64, base_branch: String, created_at: String, last_selected_at: Option<String> }`
- `Worktree { id: Option<i64>, session_id: Option<i64>, repo_id: i64, path: String, branch: String, tmux_window: Option<i64>, created_at: String }`
- `SessionInfo { id: Option<i64>, name: String, repo_id: Option<i64>, base_branch: Option<String>, created_at: Option<String>, last_selected_at: Option<String>, windows: Option<i64>, attached: bool, live: bool, managed: bool }`

### db.rs

Opens the same SQLite file. Must set `PRAGMA foreign_keys = ON` after connecting (rusqlite does not enable this by default, but the Python code requires it). Uses `conn.row_factory = sqlite3.Row` equivalent: map rows to structs via rusqlite's `query_map` / `query_row`. Runs the same CREATE TABLE IF NOT EXISTS statements. Seeds default config values (`repos_dir` = `~/dev`, `worktrees_dir` = `~/dev/worktrees`) via INSERT OR IGNORE, same as Python. Runs the same `last_selected_at` migration. Exposes the same CRUD functions (`add_repo`, `get_repos`, `add_session`, `get_sessions`, `get_session_by_name`, `touch_session`, `delete_session`, `add_worktree`, `get_worktrees`, `get_worktrees_for_session`, `delete_worktree`, `get_config`, `set_config`, `get_all_config`). All take `&Connection` instead of `sqlite3.Connection`.

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
- Properties: `repos_dir()`, `worktrees_dir()` (read from config table)

Also includes two standalone functions (not methods on Manager):
- `detect_subsystems(repo_path)` -- scans for monorepo subdirectories under `workers/`, `src/`, `libs/`, `pods/`. Used by NewSessionScreen.
- `apply_layout(session_name, working_dir)` -- creates a tmux session and sets up windows per `DEFAULT_LAYOUT` (window "claude" with `claude` command sent, window "shell" with no command). Called by `create_session()` and `checkout_and_review()`.

The `list_sessions()` enrichment logic (merge DB sessions with `tmux::list_sessions()` output, append unmanaged sessions) is identical to the Python version.

### claude_session.rs

- `get_session_id(pane_pid)` -- reads `/tmp/claude-sessions/pid-{pane_pid}`
- `get_first_user_message(session_id)` -- scans `~/.claude/projects/*/session_id.jsonl` for first user message
- `summarize_message(msg)` -- first line, first 4 words, kebab-case, max 30 chars
- `classify_pane(pane_text)` -- same regex patterns: "prompting" (detected by `❯\s+1\.` followed by "Esc to cancel" in the last 10 non-empty lines), "working" (non-ASCII character followed by a word ending in `…`), else "idle"

### conversation_index.rs

Parses `~/.claude/conversation-index.md` into structured entries. Mirrors `conversation_index.py` exactly.

- `Conversation` struct: `date: String`, `session_id: String` (short 8-char hex), `project: String`, `branch: String`, `intents: Vec<String>`, with a `summary()` method that returns the first non-interrupted intent.
- `parse_index()` -- stateful line-by-line parser using 4 regex patterns (`## YYYY-MM-DD HH:MM [hex]`, `- **project**: \`...\``, `- **branch**: \`...\``, `  - intent`). Returns entries newest-first.
- `resolve_session_id(short_id, project_path)` -- resolves a short hex ID to full UUID by scanning `~/.claude/projects/<encoded-path>/*.jsonl` where encoded-path replaces `/` with `-`.
- `filter_by_paths(entries, paths)` -- filters conversations whose project starts with any of the given paths.

### fuzzy.rs

Same scoring algorithm: characters must appear in order, consecutive matches get -1 bonus, non-consecutive matches penalized by the absolute position in the target string (not gap size). Returns `Option<i32>`.

### switch.rs

```rust
enum SwitchAction {
    Session { target: String },
    Window { session: String, window: i64 },
}
```

`write_switch(action)` serializes to `std::env::temp_dir().join("torchard-switch.json")` (must match Python's `tempfile.gettempdir()`). `read_switch()` deserializes. `cleanup()` removes the file. `write_switch` is called from multiple TUI screens (SessionList, Review, NewTab, History) -- not just main.

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
1. `terminal.draw(|f| app.render(f))` -- delegates to top screen (see Rendering)
2. `crossterm::event::poll(Duration::from_millis(50))` -- non-blocking, allows checking background channels
3. If event available: top screen's `handle_event()` returns a `ScreenAction`
4. Check `mpsc::Receiver` for background task results (see Background Work)
5. App processes actions (push, pop, quit, etc.)
6. On `Pop`, calls `on_resume()` on the new top screen (triggers table refresh, etc.)
7. On `PopWith`, calls `on_child_result()` on the new top screen with the result
8. `Quit` propagates through the entire stack -- some screens (NewSession, Review, History) call `write_switch` then quit directly, bypassing all parent screens

### Rendering

Immediate-mode rendering via ratatui. Each screen builds layout with `Layout::vertical` / `Layout::horizontal` and renders widgets. The session list uses ratatui's `Table` widget with styled `Row`/`Cell` items. Textual's rich markup (`[green]...[/green]`) becomes ratatui `Span` with `Style`.

Modal screens (ActionMenu, Confirm) are rendered as overlays. The `App::render()` method iterates the full screen stack: it renders all screens below the top (with a dimming overlay), then renders the top screen on top. This means modal screens don't need to know about their parent -- the stack handles compositing.

The footer keybinding bar is a manually rendered `Paragraph` showing available keys for the current screen.

### Color scheme

Same dark theme as the Textual CSS:
- Background: `#1a1a2e`
- Header: `#16213e` with `#00aaff` text
- Cursor row: `#0f3460` with white text
- Footer: `#16213e` with `#aaaaaa` text, keys in `#00aaff` bold
- Repo colors: same 8-color palette with collision-avoidant assignment (MD5 hash determines preferred slot, then walks forward to find first unused slot; only collides when all 8 are exhausted)

## Screen-by-Screen Notes

### SessionListScreen

The most complex screen. State: `sessions: Vec<SessionInfo>`, `repos: HashMap<i64, Repo>`, `expanded: HashSet<String>`, `filter: String`, `cursor: usize`, `filter_active: bool`.

- `refresh()` queries `manager.list_sessions()` + `tmux::list_all_windows()`, sorts/filters, rebuilds the row list
- Sort order: main pinned first, then by `last_selected_at` descending, then alphabetical
- Fuzzy filter matches against session name, repo name, and branch
- Expand/collapse shows child rows for tmux windows with Claude pane state classification
- All keybindings match: q, /, j/k/arrows, enter, tab, n, r, d, h, ., c, S, ?
- Action menu (`.`) is context-aware: session-level actions (rename, branch, launch claude, adopt) and tab-level actions (rename tab) depending on cursor position
- "Launch claude" action: creates new tmux window named "claude", sends `claude` + Enter, writes switch file, exits
- Delete (`d`) is context-aware: kills a tab when cursor is on a `win:` row, deletes the session when on a session row

### NewSessionScreen

Multi-step wizard with an enum tracking the current step:

```rust
enum WizardStep {
    PickRepo { dirs: Vec<(String, String)>, cursor: usize, filter: String, awaiting_path: bool },
    PickBranch { repo: Repo, branches: Vec<String>, cursor: usize, filter: String },
    PickSubsystem { subsystems: Vec<String>, cursor: usize, filter: String },
    // Note: no ConfirmName step -- names are auto-generated
}
```

- `PickRepo` -- filterable list of directories in `repos_dir` (excluding `worktrees_dir`), plus "+ Add new repo path..." option that switches to path entry mode
- `PickBranch` -- filterable list of branches for the selected repo, plus "+ New branch: ..." option when typed text doesn't match any branch
- `PickSubsystem` -- only shown if `detect_subsystems()` finds monorepo subdirectories. Lists subdirs under `workers/`, `src/`, `libs/`, `pods/` with a "/ (root)" option

**Auto-naming:** After branch selection, the session name is auto-generated (repo name for default branch, branch name otherwise), deduplicated by appending `-2`, `-3`, etc. The wizard skips straight to subsystem selection or session creation -- there is no manual name confirmation step in the normal flow.

Step transitions replace the enum variant. Escape goes back one step (from subsystem back to branch, skipping the name step). On `PickRepo` escape pops the screen.

After creation, the screen calls `tmux::switch_client` and exits the app (not just pops).

### HistoryScreen

Parses `~/.claude/conversation-index.md` via `conversation_index::parse_index()`. Displays a 4-column table (Date, Project, Branch, Summary) using `truncate_end` for column widths.

**Scoping:** Initialized with optional `scope_paths` (repo path + worktree paths) and `scope_label`. When scoped, only shows conversations whose project starts with one of the paths. Toggle scope on/off with `t`.

**Sorting:** 4 sort modes toggled by keybindings: `d` (date, default descending), `p` (project), `b` (branch), `s` (summary). Pressing the same key toggles ascending/descending. Date defaults to descending, others default to ascending.

**Filtering:** Substring match (not fuzzy) against all 4 fields.

**Resume logic (enter):** Resolves the short session ID to a full UUID via `resolve_session_id()`. Looks for a managed session whose repo path or worktree paths match the conversation's project. If found, creates a new tmux window in that session and sends `claude --resume <uuid>`. If not found, creates a new tmux session. In both cases, writes switch file and exits the app.

Keybindings: escape (back), j/k (navigate), t (toggle scope), d/p/b/s (sort).

### CleanupScreen

Shows **all** worktrees (not just stale ones) in a 5-column table: checkbox, branch, session name, path (`truncate_start`), status. Sorted by session name then branch.

**Background staleness check:** On mount, the table renders immediately with "checking..." status. A background thread runs `manager.get_stale_worktrees()` (which calls `git branch --merged` and `git ls-remote` for each worktree -- can take seconds). When complete, results are sent via `mpsc` channel and the main loop updates status cells to "ok" or "stale" (yellow). See Background Work section.

**Selection:** `HashSet<String>` of row keys tracks selections. `space`/`enter` toggles, `a` selects all, `A` deselects all. Status bar shows total count, stale count, and selected count.

**Deletion (`d`):** Pushes a ConfirmModal, then bulk-deletes selected worktrees via `manager.cleanup_worktree()`. Removes deleted rows from the table inline.

Keybindings: escape (back), space/enter (toggle), a/A (select/deselect all), d (delete selected), j/k (navigate).

### Simple screens

- **ReviewScreen** -- text input for PR number or branch name, tab to cycle repos (sorted by most recently active session). Checkout runs in a background thread (involves git fetch over the network). Shows "Checking out..." status while working. On success, writes switch file and exits the app. On error, shows error message inline.
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
    // Clean up stale switch file from prior crash
    switch::cleanup();

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

    // Handle deferred switch
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

## Background Work

Two screens perform work that can take seconds and must not block the UI:

- **CleanupScreen:** `get_stale_worktrees()` checks git merge status and remote branches for every worktree
- **ReviewScreen:** `checkout_and_review()` runs git fetch over the network

**Approach:** `std::thread::spawn` + `std::sync::mpsc` channel. No async runtime needed.

The `App` struct holds an `mpsc::Receiver<BackgroundResult>`. Screens that need background work send a closure to a thread and the thread sends the result back through the channel. The main event loop uses `crossterm::event::poll(Duration::from_millis(50))` instead of blocking `read()`, and checks the channel each iteration.

```rust
enum BackgroundResult {
    StaleWorktrees(Vec<Worktree>),          // for CleanupScreen
    CheckoutComplete(Result<(Session, String), String>),  // for ReviewScreen
}
```

Screens set a "loading" flag when they kick off background work and check for results in a `check_background()` method called by the App each loop iteration. This is simple, avoids an async runtime, and only affects the two screens that need it.

Note: `rusqlite::Connection` is `!Send`, so the background thread cannot share the Manager's connection. For `get_stale_worktrees`, the thread receives the worktree list and repo list as owned data, then calls git commands directly. For `checkout_and_review`, the thread can open a second read-only connection or receive the necessary data to perform git operations without DB access. The main thread handles any DB writes after the background work completes.

## Utility Functions

### utils.rs

- `truncate_end(text, max_len)` -- truncates from the right, appending `…`. Used in SessionList (repo names) and History (project, branch, summary columns).
- `truncate_start(text, max_len)` -- truncates from the left, prepending `…`. Used in Cleanup (worktree paths).

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
