# Torchard Rust TUI Port — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the Torchard TUI from Python/Textual to Rust/ratatui for near-instant startup, coexisting alongside the Python version.

**Architecture:** Screen-per-struct with stack navigation using ratatui. Each Python module maps 1:1 to a Rust module. The Rust binary reads the same SQLite database and writes the same switch file format as the Python version.

**Tech Stack:** Rust, ratatui, crossterm, rusqlite (bundled), serde/serde_json, regex, md-5, dirs

**Spec:** `docs/superpowers/specs/2026-04-07-rust-tui-port-design.md`

---

## File Structure

```
torchard-rs/
  Cargo.toml
  src/
    main.rs              # entry point
    models.rs            # Repo, Session, Worktree, SessionInfo, Conversation
    db.rs                # rusqlite CRUD, schema, migrations
    tmux.rs              # tmux CLI wrappers
    git.rs               # git/gh CLI wrappers
    manager.rs           # orchestration layer
    claude_session.rs    # pane classification, JSONL reading
    conversation_index.rs # parse conversation-index.md
    fuzzy.rs             # fuzzy match scoring
    switch.rs            # switch file read/write/cleanup
    utils.rs             # truncate_end, truncate_start
    tui/
      mod.rs             # App, Screen enum, ScreenBehavior trait, event loop
      theme.rs           # color constants, footer rendering, input widget helpers
      session_list.rs    # main list view
      new_session.rs     # multi-step wizard
      review.rs          # PR/branch checkout
      adopt_session.rs   # adopt unmanaged session
      rename.rs          # rename session + rename window
      edit_branch.rs     # change session base branch
      new_tab.rs         # create worktree tab
      action_menu.rs     # generic picker modal
      confirm.rs         # yes/no modal
      history.rs         # conversation browser
      cleanup.rs         # stale worktree manager
      settings.rs        # config editor
      help.rs            # keybinding reference
```

---

## Task 1: Project scaffolding and models

**Files:**
- Create: `torchard-rs/Cargo.toml`
- Create: `torchard-rs/src/main.rs`
- Create: `torchard-rs/src/models.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "torchard-rs"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "torchard-rs"
path = "src/main.rs"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "6"
md-5 = "0.10"
regex = "1"
```

- [ ] **Step 2: Create models.rs**

```rust
// torchard-rs/src/models.rs

#[derive(Debug, Clone)]
pub struct Repo {
    pub id: Option<i64>,
    pub path: String,
    pub name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: Option<i64>,
    pub name: String,
    pub repo_id: i64,
    pub base_branch: String,
    pub created_at: String,
    pub last_selected_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub id: Option<i64>,
    pub session_id: Option<i64>,
    pub repo_id: i64,
    pub path: String,
    pub branch: String,
    pub tmux_window: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: Option<i64>,
    pub name: String,
    pub repo_id: Option<i64>,
    pub base_branch: Option<String>,
    pub created_at: Option<String>,
    pub last_selected_at: Option<String>,
    pub windows: Option<i64>,
    pub attached: bool,
    pub live: bool,
    pub managed: bool,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub date: String,
    pub session_id: String,
    pub project: String,
    pub branch: String,
    pub intents: Vec<String>,
}

impl Conversation {
    pub fn summary(&self) -> &str {
        for intent in &self.intents {
            if !intent.starts_with("[Request interrupted") {
                return intent;
            }
        }
        self.intents.first().map(|s| s.as_str()).unwrap_or("")
    }
}
```

- [ ] **Step 3: Create minimal main.rs that compiles**

```rust
// torchard-rs/src/main.rs
mod models;

fn main() {
    println!("torchard-rs");
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cd torchard-rs && cargo build`
Expected: Compiles with no errors.

- [ ] **Step 5: Commit**

```bash
git add torchard-rs/
git commit -m "scaffold torchard-rs crate with models"
```

---

## Task 2: utils, fuzzy, and switch modules

**Files:**
- Create: `torchard-rs/src/utils.rs`
- Create: `torchard-rs/src/fuzzy.rs`
- Create: `torchard-rs/src/switch.rs`

These are standalone modules with no dependencies on other torchard code, so they're easy to test.

- [ ] **Step 1: Write utils.rs tests and implementation**

```rust
// torchard-rs/src/utils.rs

pub fn truncate_end(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_len - 1).collect();
    format!("{truncated}…")
}

pub fn truncate_start(text: &str, max_len: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_len {
        return text.to_string();
    }
    let skip = char_count - (max_len - 1);
    let truncated: String = text.chars().skip(skip).collect();
    format!("…{truncated}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_end_short() {
        assert_eq!(truncate_end("hello", 10), "hello");
    }

    #[test]
    fn truncate_end_exact() {
        assert_eq!(truncate_end("hello", 5), "hello");
    }

    #[test]
    fn truncate_end_long() {
        assert_eq!(truncate_end("hello world", 8), "hello w…");
    }

    #[test]
    fn truncate_start_short() {
        assert_eq!(truncate_start("hello", 10), "hello");
    }

    #[test]
    fn truncate_start_long() {
        assert_eq!(truncate_start("/very/long/path/here", 12), "…/path/here");
    }
}
```

- [ ] **Step 2: Write fuzzy.rs tests and implementation**

Port of `torchard/core/fuzzy.py`. The algorithm: characters in query must appear in text in order. Consecutive matches get -1 bonus. Non-consecutive matches are penalized by the absolute position in the target string.

```rust
// torchard-rs/src/fuzzy.rs

pub fn fuzzy_match(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();

    if query.is_empty() {
        return Some(0);
    }

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut last_match: i32 = -2;

    for (ti, &ch) in text.iter().enumerate() {
        if qi < query.len() && ch == query[qi] {
            let ti_i32 = ti as i32;
            if ti_i32 == last_match + 1 {
                score -= 1; // bonus for consecutive
            } else {
                score += ti_i32; // penalty for distance
            }
            last_match = ti_i32;
            qi += 1;
        }
    }

    if qi < query.len() {
        None
    } else {
        Some(score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
    }

    #[test]
    fn exact_match() {
        assert_eq!(fuzzy_match("abc", "abc"), Some(-2));
        // positions: a=0, b=1(consecutive -1), c=2(consecutive -1) => 0 + (-1) + (-1) = -2
    }

    #[test]
    fn no_match() {
        assert_eq!(fuzzy_match("xyz", "abc"), None);
    }

    #[test]
    fn partial_match_fails() {
        assert_eq!(fuzzy_match("abcd", "abc"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_match("ABC", "abc").is_some());
    }

    #[test]
    fn gap_penalized_by_position() {
        // "ac" in "abc": a=0, c=2 (not consecutive, penalty = 2) => 0 + 2 = 2
        assert_eq!(fuzzy_match("ac", "abc"), Some(2));
    }

    #[test]
    fn better_match_scores_lower() {
        let close = fuzzy_match("ab", "ab___").unwrap();
        let far = fuzzy_match("ab", "a___b").unwrap();
        assert!(close < far);
    }
}
```

- [ ] **Step 3: Write switch.rs tests and implementation**

Port of `torchard/tui/switch.py`. Uses `std::env::temp_dir()` to match Python's `tempfile.gettempdir()`.

```rust
// torchard-rs/src/switch.rs

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SwitchAction {
    #[serde(rename = "session")]
    Session { target: String },
    #[serde(rename = "window")]
    Window { session: String, window: i64 },
}

fn switch_path() -> PathBuf {
    std::env::temp_dir().join("torchard-switch.json")
}

pub fn write_switch(action: &SwitchAction) {
    let json = serde_json::to_string(action).expect("serialize switch action");
    fs::write(switch_path(), json).expect("write switch file");
}

pub fn read_switch() -> Option<SwitchAction> {
    let path = switch_path();
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn cleanup() {
    let _ = fs::remove_file(switch_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_session() {
        let action = SwitchAction::Session {
            target: "my-session".to_string(),
        };
        write_switch(&action);
        let read = read_switch().unwrap();
        match read {
            SwitchAction::Session { target } => assert_eq!(target, "my-session"),
            _ => panic!("expected Session"),
        }
        cleanup();
        assert!(read_switch().is_none());
    }

    #[test]
    fn roundtrip_window() {
        let action = SwitchAction::Window {
            session: "sess".to_string(),
            window: 3,
        };
        write_switch(&action);
        let read = read_switch().unwrap();
        match read {
            SwitchAction::Window { session, window } => {
                assert_eq!(session, "sess");
                assert_eq!(window, 3);
            }
            _ => panic!("expected Window"),
        }
        cleanup();
    }
}
```

- [ ] **Step 4: Wire up modules in main.rs and run tests**

Add `mod utils; mod fuzzy; mod switch;` to `main.rs`.

Run: `cd torchard-rs && cargo test`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add torchard-rs/
git commit -m "add utils, fuzzy, and switch modules with tests"
```

---

## Task 3: Database layer

**Files:**
- Create: `torchard-rs/src/db.rs`

Port of `torchard/core/db.py`. Same schema, same CRUD functions, same migration.

- [ ] **Step 1: Write db.rs with init, migration, and all CRUD functions**

```rust
// torchard-rs/src/db.rs

use rusqlite::{Connection, params};
use std::path::Path;

use crate::models::{Repo, Session, Worktree};

pub fn init_db(db_path: &Path) -> Connection {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(db_path).expect("open database");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS repos (
            id             INTEGER PRIMARY KEY,
            path           TEXT NOT NULL,
            name           TEXT NOT NULL,
            default_branch TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL,
            repo_id     INTEGER NOT NULL,
            base_branch TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            FOREIGN KEY (repo_id) REFERENCES repos(id)
        );
        CREATE TABLE IF NOT EXISTS worktrees (
            id          INTEGER PRIMARY KEY,
            session_id  INTEGER,
            repo_id     INTEGER NOT NULL,
            path        TEXT NOT NULL,
            branch      TEXT NOT NULL,
            tmux_window INTEGER,
            created_at  TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id),
            FOREIGN KEY (repo_id) REFERENCES repos(id)
        );
        CREATE TABLE IF NOT EXISTS config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .unwrap();

    // Seed defaults
    conn.execute(
        "INSERT OR IGNORE INTO config (key, value) VALUES (?1, ?2)",
        params![
            "repos_dir",
            dirs::home_dir().unwrap().join("dev").to_str().unwrap()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO config (key, value) VALUES (?1, ?2)",
        params![
            "worktrees_dir",
            dirs::home_dir()
                .unwrap()
                .join("dev/worktrees")
                .to_str()
                .unwrap()
        ],
    )
    .unwrap();

    // Migration: add last_selected_at column if missing
    let has_col: bool = conn
        .prepare("PRAGMA table_info(sessions)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .any(|name| name.as_deref() == Ok("last_selected_at"));
    if !has_col {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN last_selected_at TEXT;")
            .unwrap();
    }

    conn
}

// --- Config ---

pub fn get_config(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM config WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .ok()
}

pub fn set_config(conn: &Connection, key: &str, value: &str) {
    conn.execute(
        "INSERT OR REPLACE INTO config (key, value) VALUES (?1, ?2)",
        params![key, value],
    )
    .unwrap();
}

pub fn get_all_config(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM config ORDER BY key")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

// --- Repos ---

pub fn add_repo(conn: &Connection, repo: &Repo) -> Repo {
    conn.execute(
        "INSERT INTO repos (path, name, default_branch) VALUES (?1, ?2, ?3)",
        params![repo.path, repo.name, repo.default_branch],
    )
    .unwrap();
    Repo {
        id: Some(conn.last_insert_rowid()),
        ..repo.clone()
    }
}

pub fn get_repos(conn: &Connection) -> Vec<Repo> {
    let mut stmt = conn
        .prepare("SELECT id, path, name, default_branch FROM repos")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Repo {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            name: row.get(2)?,
            default_branch: row.get(3)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

// --- Sessions ---

pub fn add_session(conn: &Connection, session: &Session) -> Session {
    conn.execute(
        "INSERT INTO sessions (name, repo_id, base_branch, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            session.name,
            session.repo_id,
            session.base_branch,
            session.created_at
        ],
    )
    .unwrap();
    Session {
        id: Some(conn.last_insert_rowid()),
        ..session.clone()
    }
}

pub fn get_sessions(conn: &Connection) -> Vec<Session> {
    let mut stmt = conn
        .prepare("SELECT id, name, repo_id, base_branch, created_at, last_selected_at FROM sessions")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Session {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            repo_id: row.get(2)?,
            base_branch: row.get(3)?,
            created_at: row.get(4)?,
            last_selected_at: row.get(5)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn get_session_by_name(conn: &Connection, name: &str) -> Option<Session> {
    conn.query_row(
        "SELECT id, name, repo_id, base_branch, created_at, last_selected_at FROM sessions WHERE name = ?1",
        params![name],
        |row| {
            Ok(Session {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                repo_id: row.get(2)?,
                base_branch: row.get(3)?,
                created_at: row.get(4)?,
                last_selected_at: row.get(5)?,
            })
        },
    )
    .ok()
}

pub fn touch_session(conn: &Connection, session_id: i64) {
    let now = chrono_now();
    conn.execute(
        "UPDATE sessions SET last_selected_at = ?1 WHERE id = ?2",
        params![now, session_id],
    )
    .unwrap();
}

pub fn delete_session(conn: &Connection, session_id: i64) {
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
        .unwrap();
}

// --- Worktrees ---

pub fn add_worktree(conn: &Connection, wt: &Worktree) -> Worktree {
    conn.execute(
        "INSERT INTO worktrees (session_id, repo_id, path, branch, tmux_window, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![wt.session_id, wt.repo_id, wt.path, wt.branch, wt.tmux_window, wt.created_at],
    )
    .unwrap();
    Worktree {
        id: Some(conn.last_insert_rowid()),
        ..wt.clone()
    }
}

pub fn get_worktrees(conn: &Connection) -> Vec<Worktree> {
    let mut stmt = conn
        .prepare("SELECT id, session_id, repo_id, path, branch, tmux_window, created_at FROM worktrees")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Worktree {
            id: Some(row.get(0)?),
            session_id: row.get(1)?,
            repo_id: row.get(2)?,
            path: row.get(3)?,
            branch: row.get(4)?,
            tmux_window: row.get(5)?,
            created_at: row.get(6)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn get_worktrees_for_session(conn: &Connection, session_id: i64) -> Vec<Worktree> {
    let mut stmt = conn
        .prepare("SELECT id, session_id, repo_id, path, branch, tmux_window, created_at FROM worktrees WHERE session_id = ?1")
        .unwrap();
    stmt.query_map(params![session_id], |row| {
        Ok(Worktree {
            id: Some(row.get(0)?),
            session_id: row.get(1)?,
            repo_id: row.get(2)?,
            path: row.get(3)?,
            branch: row.get(4)?,
            tmux_window: row.get(5)?,
            created_at: row.get(6)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn delete_worktree(conn: &Connection, worktree_id: i64) {
    conn.execute(
        "DELETE FROM worktrees WHERE id = ?1",
        params![worktree_id],
    )
    .unwrap();
}

fn chrono_now() -> String {
    // UTC ISO 8601 without external crate — use std::time
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    // Format as simplified ISO 8601 — matches Python's datetime.now(timezone.utc).isoformat()
    // We'll use a basic approach; exact formatting matches Python's output enough for sorting
    let secs = duration.as_secs();
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Days since 1970-01-01
    let mut y = 1970i64;
    let mut d = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if d < md as i64 {
            m = i + 1;
            break;
        }
        d -= md as i64;
    }
    let day = d + 1;

    format!(
        "{y:04}-{m:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}+00:00",
        y = y,
        m = m,
        day = day,
        hours = hours,
        minutes = minutes,
        seconds = seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (id INTEGER PRIMARY KEY, path TEXT NOT NULL, name TEXT NOT NULL, default_branch TEXT NOT NULL);
             CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT NOT NULL, repo_id INTEGER NOT NULL, base_branch TEXT NOT NULL, created_at TEXT NOT NULL, last_selected_at TEXT, FOREIGN KEY (repo_id) REFERENCES repos(id));
             CREATE TABLE worktrees (id INTEGER PRIMARY KEY, session_id INTEGER, repo_id INTEGER NOT NULL, path TEXT NOT NULL, branch TEXT NOT NULL, tmux_window INTEGER, created_at TEXT NOT NULL, FOREIGN KEY (session_id) REFERENCES sessions(id), FOREIGN KEY (repo_id) REFERENCES repos(id));
             CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        ).unwrap();
        conn
    }

    fn make_repo(conn: &Connection) -> Repo {
        add_repo(
            conn,
            &Repo {
                id: None,
                path: "/repos/myrepo".into(),
                name: "myrepo".into(),
                default_branch: "main".into(),
            },
        )
    }

    #[test]
    fn add_and_get_repos() {
        let conn = test_conn();
        let repo = make_repo(&conn);
        assert!(repo.id.is_some());
        let repos = get_repos(&conn);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "myrepo");
    }

    #[test]
    fn add_and_get_sessions() {
        let conn = test_conn();
        let repo = make_repo(&conn);
        let session = add_session(
            &conn,
            &Session {
                id: None,
                name: "test-session".into(),
                repo_id: repo.id.unwrap(),
                base_branch: "main".into(),
                created_at: "2026-01-01T00:00:00".into(),
                last_selected_at: None,
            },
        );
        assert!(session.id.is_some());
        let sessions = get_sessions(&conn);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "test-session");
    }

    #[test]
    fn get_session_by_name_found() {
        let conn = test_conn();
        let repo = make_repo(&conn);
        add_session(
            &conn,
            &Session {
                id: None,
                name: "findme".into(),
                repo_id: repo.id.unwrap(),
                base_branch: "main".into(),
                created_at: "2026-01-01T00:00:00".into(),
                last_selected_at: None,
            },
        );
        assert!(get_session_by_name(&conn, "findme").is_some());
        assert!(get_session_by_name(&conn, "nope").is_none());
    }

    #[test]
    fn touch_session_updates_timestamp() {
        let conn = test_conn();
        let repo = make_repo(&conn);
        let session = add_session(
            &conn,
            &Session {
                id: None,
                name: "s".into(),
                repo_id: repo.id.unwrap(),
                base_branch: "main".into(),
                created_at: "2026-01-01T00:00:00".into(),
                last_selected_at: None,
            },
        );
        assert!(session.last_selected_at.is_none());
        touch_session(&conn, session.id.unwrap());
        let updated = get_session_by_name(&conn, "s").unwrap();
        assert!(updated.last_selected_at.is_some());
    }

    #[test]
    fn delete_session_removes_it() {
        let conn = test_conn();
        let repo = make_repo(&conn);
        let session = add_session(
            &conn,
            &Session {
                id: None,
                name: "del".into(),
                repo_id: repo.id.unwrap(),
                base_branch: "main".into(),
                created_at: "2026-01-01T00:00:00".into(),
                last_selected_at: None,
            },
        );
        delete_session(&conn, session.id.unwrap());
        assert!(get_sessions(&conn).is_empty());
    }

    #[test]
    fn add_and_get_worktrees() {
        let conn = test_conn();
        let repo = make_repo(&conn);
        let session = add_session(
            &conn,
            &Session {
                id: None,
                name: "s".into(),
                repo_id: repo.id.unwrap(),
                base_branch: "main".into(),
                created_at: "2026-01-01T00:00:00".into(),
                last_selected_at: None,
            },
        );
        let wt = add_worktree(
            &conn,
            &Worktree {
                id: None,
                session_id: session.id,
                repo_id: repo.id.unwrap(),
                path: "/wt/feat".into(),
                branch: "feat".into(),
                tmux_window: None,
                created_at: "2026-01-01T00:00:00".into(),
            },
        );
        assert!(wt.id.is_some());
        let all = get_worktrees(&conn);
        assert_eq!(all.len(), 1);
        let for_session = get_worktrees_for_session(&conn, session.id.unwrap());
        assert_eq!(for_session.len(), 1);
    }

    #[test]
    fn config_crud() {
        let conn = test_conn();
        assert!(get_config(&conn, "foo").is_none());
        set_config(&conn, "foo", "bar");
        assert_eq!(get_config(&conn, "foo").unwrap(), "bar");
        set_config(&conn, "foo", "baz");
        assert_eq!(get_config(&conn, "foo").unwrap(), "baz");
    }

    #[test]
    fn init_db_creates_schema() {
        let tmp = std::env::temp_dir().join("torchard-test-init.db");
        let _ = std::fs::remove_file(&tmp);
        let conn = init_db(&tmp);
        // Verify tables exist
        let repos = get_repos(&conn);
        assert!(repos.is_empty());
        // Verify default config
        assert!(get_config(&conn, "repos_dir").is_some());
        assert!(get_config(&conn, "worktrees_dir").is_some());
        let _ = std::fs::remove_file(&tmp);
    }
}
```

- [ ] **Step 2: Wire up in main.rs and run tests**

Add `mod db;` to `main.rs`.

Run: `cd torchard-rs && cargo test`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add database layer with schema, migrations, and CRUD"
```

---

## Task 4: tmux and git CLI wrappers

**Files:**
- Create: `torchard-rs/src/tmux.rs`
- Create: `torchard-rs/src/git.rs`

These are thin wrappers around `Command::new`. Same functions as the Python versions. Not unit-testable without tmux/git, so we verify they compile and test them manually later.

- [ ] **Step 1: Write tmux.rs**

Port every function from `torchard/core/tmux.py`. Use `std::process::Command` with `output()`. Return `Result<T, TmuxError>`. Key functions:

```rust
// torchard-rs/src/tmux.rs

use std::collections::HashMap;
use std::process::Command;

#[derive(Debug)]
pub struct TmuxError(pub String);

impl std::fmt::Display for TmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TmuxError {}

pub fn sanitize_session_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c == '.' || c == ':' { '-' } else { c })
        .collect();
    let trimmed = sanitized.trim_matches(|c: char| c == ' ' || c == '-');
    if trimmed.is_empty() {
        "new-session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new("tmux")
        .args(args)
        .output()
        .expect("failed to execute tmux")
}

#[derive(Debug, Clone)]
pub struct TmuxSession {
    pub name: String,
    pub windows: i64,
    pub attached: bool,
}

pub fn list_sessions() -> Vec<TmuxSession> {
    let output = run(&[
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_windows}\t#{session_attached}",
    ]);
    if !output.status.success() {
        return vec![];
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                return None;
            }
            Some(TmuxSession {
                name: parts[0].to_string(),
                windows: parts[1].parse().unwrap_or(0),
                attached: parts[2] == "1",
            })
        })
        .collect()
}

pub fn session_exists(name: &str) -> bool {
    run(&["has-session", "-t", name]).status.success()
}

pub fn new_session(name: &str, start_dir: &str) -> Result<(), TmuxError> {
    if session_exists(name) {
        return Err(TmuxError(format!("Session '{}' already exists", name)));
    }
    let output = run(&["new-session", "-d", "-s", name, "-c", start_dir]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to create session '{}': {}",
            name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn switch_client(target: &str) -> Result<(), TmuxError> {
    let output = run(&["switch-client", "-t", target]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to switch to '{}': {}",
            target,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn new_window(session: &str, name: &str, start_dir: Option<&str>) -> Result<(), TmuxError> {
    let mut args = vec!["new-window", "-t", session, "-n", name];
    if let Some(dir) = start_dir {
        args.extend_from_slice(&["-c", dir]);
    }
    let output = run(&args);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to create window '{}' in '{}': {}",
            name,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn select_window(session: &str, index: i64) -> Result<(), TmuxError> {
    let target = format!("{}:{}", session, index);
    let output = run(&["select-window", "-t", &target]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to select window {} in '{}': {}",
            index,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn rename_window(session: &str, index: i64, new_name: &str) -> Result<(), TmuxError> {
    let target = format!("{}:{}", session, index);
    let output = run(&["rename-window", "-t", &target, new_name]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to rename window {} in '{}': {}",
            index,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn rename_session(old_name: &str, new_name: &str) -> Result<(), TmuxError> {
    let output = run(&["rename-session", "-t", old_name, new_name]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to rename session '{}' to '{}': {}",
            old_name,
            new_name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct TmuxWindow {
    pub index: i64,
    pub name: String,
    pub path: String,
    pub command: String,
    pub pane_pid: String,
}

pub fn list_all_windows() -> HashMap<String, Vec<TmuxWindow>> {
    let output = run(&[
        "list-windows",
        "-a",
        "-F",
        "#{session_name}\t#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}",
    ]);
    if !output.status.success() {
        return HashMap::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut by_session: HashMap<String, Vec<TmuxWindow>> = HashMap::new();
    for line in stdout.trim().lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let session = parts[0].to_string();
        let win = TmuxWindow {
            index: parts[1].parse().unwrap_or(0),
            name: parts[2].to_string(),
            path: parts[3].to_string(),
            command: parts.get(4).unwrap_or(&"").to_string(),
            pane_pid: parts.get(5).unwrap_or(&"").to_string(),
        };
        by_session.entry(session).or_default().push(win);
    }
    by_session
}

pub fn list_windows(session: &str) -> Vec<TmuxWindow> {
    let output = run(&[
        "list-windows",
        "-t",
        session,
        "-F",
        "#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}",
    ]);
    if !output.status.success() {
        return vec![];
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                return None;
            }
            Some(TmuxWindow {
                index: parts[0].parse().unwrap_or(0),
                name: parts[1].to_string(),
                path: parts[2].to_string(),
                command: parts.get(3).unwrap_or(&"").to_string(),
                pane_pid: parts.get(4).unwrap_or(&"").to_string(),
            })
        })
        .collect()
}

pub fn kill_window(session: &str, index: i64) -> Result<(), TmuxError> {
    let target = format!("{}:{}", session, index);
    let output = run(&["kill-window", "-t", &target]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to kill window {} in '{}': {}",
            index,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn send_keys(target: &str, keys: &[&str]) {
    let mut args = vec!["send-keys", "-t", target];
    args.extend_from_slice(keys);
    run(&args);
}

pub fn capture_pane(target: &str, lines: i64) -> String {
    let lines_arg = format!("-{}", lines);
    let output = run(&["capture-pane", "-t", target, "-p", "-J", "-S", &lines_arg]);
    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::new()
    }
}

pub fn get_pane_pid(target: &str) -> Option<String> {
    let output = run(&["display-message", "-t", target, "-p", "#{pane_pid}"]);
    if output.status.success() {
        let pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if pid.is_empty() {
            None
        } else {
            Some(pid)
        }
    } else {
        None
    }
}

pub fn kill_session(name: &str) -> Result<(), TmuxError> {
    let output = run(&["kill-session", "-t", name]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to kill session '{}': {}",
            name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_dots_and_colons() {
        assert_eq!(sanitize_session_name("foo.bar:baz"), "foo-bar-baz");
    }

    #[test]
    fn sanitize_trims() {
        assert_eq!(sanitize_session_name(" -hello- "), "hello");
    }

    #[test]
    fn sanitize_empty_fallback() {
        assert_eq!(sanitize_session_name("..."), "new-session");
    }
}
```

- [ ] **Step 2: Write git.rs**

Port every function from `torchard/core/git.py`. Same pattern: `Command::new("git")` / `Command::new("gh")`.

```rust
// torchard-rs/src/git.rs

use std::process::Command;

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

fn run(args: &[&str], cwd: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output().expect("failed to execute git")
}

pub fn detect_default_branch(repo_path: &str) -> Result<String, GitError> {
    let output = run(
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
        Some(repo_path),
    );
    if output.status.success() {
        let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(branch) = refname.rsplit('/').next() {
            return Ok(branch.to_string());
        }
    }
    let output = run(&["branch", "--format=%(refname:short)"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to list branches in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let branches: Vec<&str> = String::from_utf8_lossy(&output.stdout)
        .trim()
        .lines()
        .collect();
    for candidate in &["main", "master"] {
        if branches.contains(candidate) {
            return Ok(candidate.to_string());
        }
    }
    Err(GitError(format!(
        "Could not determine default branch in '{}'",
        repo_path
    )))
}

pub fn list_branches(repo_path: &str) -> Result<Vec<String>, GitError> {
    let output = run(&["branch", "--format=%(refname:short)"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to list branches in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

#[derive(Debug)]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
}

pub fn list_worktrees(repo_path: &str) -> Result<Vec<GitWorktree>, GitError> {
    let output = run(&["worktree", "list", "--porcelain"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to list worktrees in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(GitWorktree {
                path: path.to_string(),
                branch: None,
                commit: None,
            });
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            if let Some(ref mut wt) = current {
                wt.commit = Some(head.to_string());
            }
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if let Some(ref mut wt) = current {
                wt.branch = Some(
                    branch_ref
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch_ref)
                        .to_string(),
                );
            }
        } else if line == "detached" {
            if let Some(ref mut wt) = current {
                wt.branch = Some("(detached)".to_string());
            }
        }
    }
    if let Some(wt) = current {
        worktrees.push(wt);
    }
    Ok(worktrees)
}

pub fn create_worktree(
    repo_path: &str,
    worktree_path: &str,
    branch: &str,
    base_branch: &str,
) -> Result<(), GitError> {
    // Check if path is already in use
    let existing = list_worktrees(repo_path)?;
    for wt in &existing {
        if wt.path == worktree_path {
            return Err(GitError(format!(
                "Worktree path '{}' is already in use",
                worktree_path
            )));
        }
    }
    let output = run(
        &["worktree", "add", "-b", branch, worktree_path, base_branch],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to create worktree at '{}' (branch '{}' from '{}'): {}",
            worktree_path,
            branch,
            base_branch,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn remove_worktree(repo_path: &str, worktree_path: &str) -> Result<(), GitError> {
    let output = run(&["worktree", "remove", worktree_path], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to remove worktree '{}': {}",
            worktree_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn get_pr_branch(repo_path: &str, pr_number: i64) -> Result<String, GitError> {
    let pr_str = pr_number.to_string();
    let output = Command::new("gh")
        .args(["pr", "view", &pr_str, "--json", "headRefName", "--jq", ".headRefName"])
        .current_dir(repo_path)
        .output()
        .expect("failed to execute gh");
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to get PR #{}: {}",
            pr_number,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err(GitError(format!("PR #{} has no head branch", pr_number)));
    }
    Ok(branch)
}

pub fn fetch_and_pull(repo_path: &str, branch: &str) {
    run(&["fetch", "origin"], Some(repo_path));
    run(&["pull", "origin", branch], Some(repo_path));
}

pub fn fetch_branch(repo_path: &str, branch: &str) {
    run(&["fetch", "origin", branch], Some(repo_path));
}

pub fn is_branch_merged(repo_path: &str, branch: &str, into: &str) -> Result<bool, GitError> {
    let output = run(
        &["branch", "--merged", into, "--format=%(refname:short)"],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to check merged branches in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let merged: Vec<&str> = String::from_utf8_lossy(&output.stdout)
        .trim()
        .lines()
        .collect();
    Ok(merged.contains(&branch))
}

pub fn has_remote_branch(repo_path: &str, branch: &str) -> Result<bool, GitError> {
    let output = run(
        &["ls-remote", "--heads", "origin", branch],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to query remote for branch '{}' in '{}': {}",
            branch,
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}
```

- [ ] **Step 3: Wire up in main.rs and verify compilation**

Add `mod tmux; mod git;` to `main.rs`.

Run: `cd torchard-rs && cargo build`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add torchard-rs/
git commit -m "add tmux and git CLI wrapper modules"
```

---

## Task 5: claude_session and conversation_index modules

**Files:**
- Create: `torchard-rs/src/claude_session.rs`
- Create: `torchard-rs/src/conversation_index.rs`

- [ ] **Step 1: Write claude_session.rs with tests**

Port of `torchard/core/claude_session.py`.

```rust
// torchard-rs/src/claude_session.rs

use regex::Regex;
use std::fs;
use std::path::Path;

pub fn get_session_id(pane_pid: &str) -> Option<String> {
    if pane_pid.is_empty() {
        return None;
    }
    let pid_file = Path::new("/tmp/claude-sessions").join(format!("pid-{}", pane_pid));
    fs::read_to_string(pid_file).ok().map(|s| s.trim().to_string())
}

pub fn get_first_user_message(session_id: &str) -> Option<String> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    if !projects_dir.exists() {
        return None;
    }
    for entry in fs::read_dir(&projects_dir).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let jsonl = entry.path().join(format!("{}.jsonl", session_id));
        if jsonl.exists() {
            return first_user_message_from_jsonl(&jsonl);
        }
    }
    None
}

fn first_user_message_from_jsonl(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let entry: serde_json::Value = serde_json::from_str(line).ok()?;
        if entry.get("type")?.as_str()? == "user" {
            let content = entry.get("message")?.get("content")?.as_str()?;
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn summarize_message(message: &str, max_words: usize) -> String {
    let first_line = message.lines().next().unwrap_or("").trim().trim_start_matches('#').trim();
    let words: Vec<&str> = first_line.split_whitespace().take(max_words).collect();
    let name: String = words
        .iter()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| ".,!?:;\"'()[]{}".contains(c))
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if name.len() > 30 {
        if let Some(pos) = name[..30].rfind('-') {
            return name[..pos].to_string();
        }
        return name[..30].to_string();
    }
    if name.is_empty() {
        "claude".to_string()
    } else {
        name
    }
}

pub fn classify_pane(pane_text: &str) -> &'static str {
    let lines: Vec<&str> = pane_text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: &[&str] = if lines.len() > 10 {
        &lines[lines.len() - 10..]
    } else {
        &lines
    };
    if tail.is_empty() {
        return "idle";
    }
    let tail_text: String = tail.join("\n");

    // Permission dialog: ❯ followed by numbered choice + "Esc to cancel"
    let prompting_re = Regex::new(r"❯\s+1\.").unwrap();
    if prompting_re.is_match(&tail_text) && tail_text.contains("Esc to cancel") {
        return "prompting";
    }

    // Active spinner: non-ASCII char followed by word ending in …
    let working_re = Regex::new(r"[^\x00-\x7f]\s+\S+…").unwrap();
    if working_re.is_match(&tail_text) {
        return "working";
    }

    "idle"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_basic() {
        assert_eq!(summarize_message("Fix the login bug", 4), "fix-the-login-bug");
    }

    #[test]
    fn summarize_truncates_words() {
        assert_eq!(
            summarize_message("one two three four five six", 4),
            "one-two-three-four"
        );
    }

    #[test]
    fn summarize_strips_punctuation() {
        assert_eq!(summarize_message("Hello, world!", 4), "hello-world");
    }

    #[test]
    fn summarize_empty() {
        assert_eq!(summarize_message("", 4), "claude");
    }

    #[test]
    fn summarize_strips_heading() {
        assert_eq!(summarize_message("## Fix the bug", 4), "fix-the-bug");
    }

    #[test]
    fn classify_idle_empty() {
        assert_eq!(classify_pane(""), "idle");
    }

    #[test]
    fn classify_working() {
        assert_eq!(classify_pane("✻ Envisioning…"), "working");
    }

    #[test]
    fn classify_prompting() {
        let text = "some output\n❯ 1. Yes\n  2. No\nEsc to cancel";
        assert_eq!(classify_pane(text), "prompting");
    }

    #[test]
    fn classify_idle_normal() {
        assert_eq!(classify_pane("❯ some command\noutput"), "idle");
    }
}
```

- [ ] **Step 2: Write conversation_index.rs with tests**

Port of `torchard/core/conversation_index.py`.

```rust
// torchard-rs/src/conversation_index.rs

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

use crate::models::Conversation;

pub fn index_path() -> PathBuf {
    dirs::home_dir()
        .unwrap()
        .join(".claude")
        .join("conversation-index.md")
}

pub fn parse_index(path: Option<&Path>) -> Vec<Conversation> {
    let path = path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(index_path);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let header_re = Regex::new(r"^## (\d{4}-\d{2}-\d{2} \d{2}:\d{2}) \[([0-9a-f]+)\]").unwrap();
    let project_re = Regex::new(r"^- \*\*project\*\*: `(.+)`").unwrap();
    let branch_re = Regex::new(r"^- \*\*branch\*\*: `(.+)`").unwrap();
    let intent_re = Regex::new(r"^  - (.+)").unwrap();

    let mut entries: Vec<Conversation> = Vec::new();
    let mut current: Option<Conversation> = None;
    let mut in_intent = false;

    for line in content.lines() {
        if let Some(caps) = header_re.captures(line) {
            if let Some(conv) = current.take() {
                entries.push(conv);
            }
            current = Some(Conversation {
                date: caps[1].to_string(),
                session_id: caps[2].to_string(),
                project: String::new(),
                branch: String::new(),
                intents: Vec::new(),
            });
            in_intent = false;
            continue;
        }

        let cur = match current.as_mut() {
            Some(c) => c,
            None => continue,
        };

        if let Some(caps) = project_re.captures(line) {
            cur.project = caps[1].to_string();
            in_intent = false;
            continue;
        }

        if let Some(caps) = branch_re.captures(line) {
            cur.branch = caps[1].to_string();
            in_intent = false;
            continue;
        }

        if line.trim() == "- **intent**:" {
            in_intent = true;
            continue;
        }

        if in_intent {
            if let Some(caps) = intent_re.captures(line) {
                cur.intents.push(caps[1].to_string());
                continue;
            } else {
                in_intent = false;
            }
        }
    }

    if let Some(conv) = current {
        entries.push(conv);
    }

    entries.reverse();
    entries
}

pub fn resolve_session_id(short_id: &str, project_path: &str) -> String {
    let encoded = project_path.replace('/', "-");
    let projects_dir = dirs::home_dir()
        .unwrap()
        .join(".claude")
        .join("projects")
        .join(&encoded);
    if projects_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if stem.starts_with(short_id) {
                            return stem.to_string();
                        }
                    }
                }
            }
        }
    }
    short_id.to_string()
}

pub fn filter_by_paths(entries: &[Conversation], paths: &[String]) -> Vec<Conversation> {
    entries
        .iter()
        .filter(|e| paths.iter().any(|p| e.project.starts_with(p)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_index() -> String {
        r#"## 2026-04-01 10:00 [abc12345]
- **project**: `/home/user/dev/myrepo`
- **branch**: `main`
- **intent**:
  - Fix the login bug
  - Add tests

## 2026-03-30 14:30 [def67890]
- **project**: `/home/user/dev/other`
- **branch**: `feature`
- **intent**:
  - Refactor auth module
"#
        .to_string()
    }

    #[test]
    fn parse_index_entries() {
        let tmp = std::env::temp_dir().join("torchard-test-index.md");
        let mut f = fs::File::create(&tmp).unwrap();
        f.write_all(sample_index().as_bytes()).unwrap();

        let entries = parse_index(Some(&tmp));
        assert_eq!(entries.len(), 2);

        // Newest first
        assert_eq!(entries[0].session_id, "abc12345");
        assert_eq!(entries[0].date, "2026-04-01 10:00");
        assert_eq!(entries[0].project, "/home/user/dev/myrepo");
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].intents.len(), 2);
        assert_eq!(entries[0].summary(), "Fix the login bug");

        assert_eq!(entries[1].session_id, "def67890");

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn parse_index_missing_file() {
        let entries = parse_index(Some(Path::new("/nonexistent/file.md")));
        assert!(entries.is_empty());
    }

    #[test]
    fn filter_by_paths_works() {
        let entries = vec![
            Conversation {
                date: "d".into(),
                session_id: "a".into(),
                project: "/home/user/dev/myrepo".into(),
                branch: "main".into(),
                intents: vec![],
            },
            Conversation {
                date: "d".into(),
                session_id: "b".into(),
                project: "/home/user/dev/other".into(),
                branch: "main".into(),
                intents: vec![],
            },
        ];
        let filtered = filter_by_paths(&entries, &["/home/user/dev/myrepo".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].session_id, "a");
    }
}
```

- [ ] **Step 3: Wire up and run tests**

Add `mod claude_session; mod conversation_index;` to `main.rs`.

Run: `cd torchard-rs && cargo test`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add torchard-rs/
git commit -m "add claude_session and conversation_index modules with tests"
```

---

## Task 6: Manager module

**Files:**
- Create: `torchard-rs/src/manager.rs`

Port of `torchard/core/manager.py`. The Manager ties DB, tmux, and git together.

- [ ] **Step 1: Write manager.rs**

This is a large file — port every method from the Python Manager class plus the standalone `detect_subsystems` and `apply_layout` functions. Reference `torchard/core/manager.py` for the exact logic of each method.

Key points:
- `Manager` holds `rusqlite::Connection`
- `repos_dir()` / `worktrees_dir()` read from config, with same defaults as Python
- `create_session()` handles both default-branch (in-place) and feature-branch (worktree) cases
- `list_sessions()` merges DB sessions with `tmux::list_sessions()` output
- `scan_existing()` has 3 sub-scans: repos, worktrees, tmux sessions
- `apply_layout()` creates tmux session + "claude"/"shell" windows
- `detect_subsystems()` checks `workers/`, `src/`, `libs/`, `pods/` for subdirectories

The file will be ~350-400 lines. Port method-by-method from the Python source.

- [ ] **Step 2: Wire up and verify compilation**

Add `mod manager;` to `main.rs`.

Run: `cd torchard-rs && cargo build`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add manager module with full orchestration logic"
```

---

## Task 7: TUI framework — App, Screen trait, event loop, theme

**Files:**
- Create: `torchard-rs/src/tui/mod.rs`
- Create: `torchard-rs/src/tui/theme.rs`

This is the TUI infrastructure that all screens depend on.

- [ ] **Step 1: Write theme.rs with color constants and helper functions**

```rust
// torchard-rs/src/tui/theme.rs

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(0x1a, 0x1a, 0x2e);
pub const HEADER_BG: Color = Color::Rgb(0x16, 0x21, 0x3e);
pub const ACCENT: Color = Color::Rgb(0x00, 0xaa, 0xff);
pub const CURSOR_BG: Color = Color::Rgb(0x0f, 0x34, 0x60);
pub const TEXT: Color = Color::Rgb(0xe0, 0xe0, 0xe0);
pub const TEXT_DIM: Color = Color::Rgb(0xaa, 0xaa, 0xaa);
pub const RED: Color = Color::Rgb(0xff, 0x6b, 0x6b);
pub const GREEN: Color = Color::Rgb(0x51, 0xcf, 0x66);
pub const YELLOW: Color = Color::Rgb(0xff, 0xd4, 0x3b);
pub const ORANGE: Color = Color::Rgb(0xe8, 0x7b, 0x35);
pub const PURPLE: Color = Color::Rgb(0xcc, 0x5d, 0xe8);
pub const CYAN: Color = Color::Rgb(0x22, 0xb8, 0xcf);
pub const PINK: Color = Color::Rgb(0xf0, 0x65, 0x95);
pub const BLUE: Color = Color::Rgb(0x00, 0xaa, 0xff);

pub const REPO_COLORS: [Color; 8] = [
    BLUE,
    RED,
    GREEN,
    YELLOW,
    PURPLE,
    Color::Rgb(0xff, 0x92, 0x2b), // orange
    CYAN,
    PINK,
];

pub fn style_default() -> Style {
    Style::default().fg(TEXT).bg(BG)
}

pub fn style_header() -> Style {
    Style::default().fg(ACCENT).bg(HEADER_BG).add_modifier(Modifier::BOLD)
}

pub fn style_cursor() -> Style {
    Style::default().fg(Color::White).bg(CURSOR_BG)
}

pub fn style_footer() -> Style {
    Style::default().fg(TEXT_DIM).bg(HEADER_BG)
}

pub fn style_footer_key() -> Style {
    Style::default().fg(ACCENT).bg(HEADER_BG).add_modifier(Modifier::BOLD)
}

pub fn style_dim() -> Style {
    Style::default().fg(TEXT_DIM)
}

pub fn style_error() -> Style {
    Style::default().fg(RED)
}

pub fn style_green() -> Style {
    Style::default().fg(GREEN)
}
```

- [ ] **Step 2: Write tui/mod.rs with App struct, Screen enum, ScreenBehavior trait, and event loop**

```rust
// torchard-rs/src/tui/mod.rs

pub mod theme;
pub mod session_list;
pub mod action_menu;
pub mod confirm;
pub mod help;
// Additional screen modules will be added in later tasks

use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{prelude::*, widgets::*};

use crate::manager::Manager;
use crate::models::*;
use crate::switch::{self, SwitchAction};

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

pub enum BackgroundResult {
    StaleWorktrees(Vec<Worktree>),
    CheckoutComplete(Result<(Session, String), String>),
}

pub trait ScreenBehavior {
    fn render(&self, f: &mut Frame, area: Rect, manager: &Manager);
    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction;
    fn on_child_result(&mut self, _result: ActionResult, _manager: &mut Manager) -> ScreenAction {
        ScreenAction::None
    }
    fn on_resume(&mut self, _manager: &mut Manager) {}
    fn on_background_result(&mut self, _result: BackgroundResult, _manager: &mut Manager) -> ScreenAction {
        ScreenAction::None
    }
    fn is_modal(&self) -> bool {
        false
    }
}

// All screen types — will grow as we add modules
pub enum Screen {
    SessionList(session_list::SessionListScreen),
    ActionMenu(action_menu::ActionMenuScreen),
    Confirm(confirm::ConfirmScreen),
    Help(help::HelpScreen),
    // Remaining screens added in later tasks
}

impl Screen {
    fn behavior(&self) -> &dyn ScreenBehavior {
        match self {
            Screen::SessionList(s) => s,
            Screen::ActionMenu(s) => s,
            Screen::Confirm(s) => s,
            Screen::Help(s) => s,
        }
    }

    fn behavior_mut(&mut self) -> &mut dyn ScreenBehavior {
        match self {
            Screen::SessionList(s) => s,
            Screen::ActionMenu(s) => s,
            Screen::Confirm(s) => s,
            Screen::Help(s) => s,
        }
    }
}

pub struct App {
    pub manager: Manager,
    screen_stack: Vec<Screen>,
    should_quit: bool,
    bg_rx: mpsc::Receiver<BackgroundResult>,
    pub bg_tx: mpsc::Sender<BackgroundResult>,
}

impl App {
    pub fn new(manager: Manager) -> Self {
        let (bg_tx, bg_rx) = mpsc::channel();
        Self {
            manager,
            screen_stack: Vec::new(),
            should_quit: false,
            bg_rx,
            bg_tx,
        }
    }

    pub fn push(&mut self, screen: Screen) {
        self.screen_stack.push(screen);
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) {
        // Push initial screen
        let initial = session_list::SessionListScreen::new(&self.manager);
        self.screen_stack.push(Screen::SessionList(initial));

        while !self.should_quit {
            // Draw
            terminal
                .draw(|f| self.render(f))
                .expect("draw");

            // Check for background results
            while let Ok(result) = self.bg_rx.try_recv() {
                if let Some(top) = self.screen_stack.last_mut() {
                    let action = top.behavior_mut().on_background_result(result, &mut self.manager);
                    self.process_action(action);
                }
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
                // Render parent of a modal (dimmed)
                screen.behavior().render(f, area, &self.manager);
                // Dim overlay
                let dim = Block::default().style(Style::default().bg(Color::Rgb(0, 0, 0)));
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
```

- [ ] **Step 3: Wire up TUI module in main.rs**

```rust
// Update main.rs
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

    let data_dir = dirs::data_dir().expect("no data dir");
    let db_path = data_dir.join("torchard").join("torchard.db");
    let first_run = !db_path.exists();
    let conn = db::init_db(&db_path);
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
```

- [ ] **Step 4: Verify compilation (will need stub screen modules)**

Create stub files for `session_list.rs`, `action_menu.rs`, `confirm.rs`, `help.rs` with empty structs implementing ScreenBehavior. These will be fleshed out in the next tasks.

Run: `cd torchard-rs && cargo build`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add torchard-rs/
git commit -m "add TUI framework with App, Screen trait, and event loop"
```

---

## Task 8: ActionMenu and ConfirmModal screens

**Files:**
- Create: `torchard-rs/src/tui/action_menu.rs`
- Create: `torchard-rs/src/tui/confirm.rs`

These are reusable modals used by many other screens, so they come first.

- [ ] **Step 1: Write action_menu.rs**

Port of `torchard/tui/views/action_menu.py`. A list picker modal that returns the selected key via `PopWith(MenuPick(...))`.

Renders: centered box with title + list items. j/k navigate, enter selects, escape cancels. Items are `Vec<(String, String, String)>` = (key, label, hint).

- [ ] **Step 2: Write confirm.rs**

Port of `torchard/tui/views/confirm.py`. Yes/no modal that returns `PopWith(Confirmed(bool))`.

Renders: centered box with title, body text, [y] Yes / [n] No buttons. y confirms, n/escape cancels.

Both screens implement `is_modal() -> true` so the App renders the parent dimmed underneath.

- [ ] **Step 3: Verify compilation**

Run: `cd torchard-rs && cargo build`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add torchard-rs/
git commit -m "add ActionMenu and ConfirmModal screens"
```

---

## Task 9: HelpScreen

**Files:**
- Create: `torchard-rs/src/tui/help.rs`

- [ ] **Step 1: Write help.rs**

Port of `torchard/tui/views/help.py`. Static keybinding reference. Renders the same help text as `_HELP_TEXT` in the Python file, using ratatui Spans with styled colors to match.

Keybindings: escape or q to dismiss (Pop).

Implements `is_modal() -> true`.

- [ ] **Step 2: Verify compilation**

Run: `cd torchard-rs && cargo build`

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add HelpScreen"
```

---

## Task 10: SessionListScreen — core rendering and navigation

**Files:**
- Create: `torchard-rs/src/tui/session_list.rs`

This is the biggest screen. Build it incrementally: first render + navigation, then actions in the next task.

- [ ] **Step 1: Write SessionListScreen with state, refresh, render, and basic navigation**

Port the core of `torchard/tui/views/session_list.py`:

- State: `sessions: Vec<SessionInfo>`, `repos: HashMap<i64, Repo>`, `expanded: HashSet<String>`, `filter: String`, `filter_active: bool`, `cursor: usize`, `rows: Vec<RowData>` (pre-computed row data for rendering)
- `refresh()` -- calls `manager.list_sessions()` + `tmux::list_all_windows()`, applies `_sorted_sessions` logic, builds row data
- `render()` -- ratatui `Table` with 3 columns (Session, Repo, Branch), styled per the color scheme. Footer with keybindings. Filter input at top when active.
- `_sorted_sessions()` -- same sort logic: main pinned first, then `last_selected_at` descending, then alphabetical
- `_assign_repo_colors()` -- collision-avoidant MD5 hash assignment, port from `session_list.py` lines 34-53
- Expand/collapse child rows for tmux windows with Claude pane state classification
- Navigation: j/k (cursor), enter (switch), tab (expand), / (filter), q/escape (quit)

This is a large file (~400-500 lines). Reference the Python source closely for the exact rendering logic — each row format, status indicators (● ○ ◇), expand arrows (▾ ▸), repo color grouping, etc.

- [ ] **Step 2: Manual test**

Run: `cd torchard-rs && cargo run`
Expected: Session list renders with correct data from the existing SQLite database. Navigation works. Filter works. Expand/collapse works.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add SessionListScreen with rendering and navigation"
```

---

## Task 11: SessionListScreen — actions (new, delete, action menu, history, etc.)

**Files:**
- Modify: `torchard-rs/src/tui/session_list.rs`

Add all the action keybindings that push other screens.

- [ ] **Step 1: Add action handlers**

Port the action methods from `session_list.py`:

- `n` (new picker) -- push ActionMenu with "New session" / "New tab in {name}" options. On result, push NewSessionScreen or NewTabScreen.
- `r` (review) -- push ReviewScreen
- `d` (delete) -- context-aware: push ConfirmModal for session delete or tab kill. On confirm, call `manager.delete_session()` or `tmux::kill_window()`.
- `.` (action menu) -- context-aware: session-level (rename, branch, launch claude, adopt) or tab-level (rename tab). On result, push appropriate screen or execute action.
- `h` (history) -- push HistoryScreen with scope_paths from current session's repo
- `c` (cleanup) -- push CleanupScreen
- `S` (settings) -- push SettingsScreen
- `?` (help) -- push HelpScreen

For screens not yet implemented, add `todo!()` placeholders in the match arms — they'll be filled in by later tasks. Or alternatively, add them to the Screen enum now as empty stubs.

- [ ] **Step 2: Add all remaining screen variants to the Screen enum in tui/mod.rs**

Add: `NewSession`, `Review`, `AdoptSession`, `Rename`, `RenameWindow`, `EditBranch`, `NewTab`, `History`, `Cleanup`, `Settings`.

Create stub files for each with minimal `ScreenBehavior` implementations that just pop on escape.

- [ ] **Step 3: Verify compilation and test basic actions**

Run: `cd torchard-rs && cargo run`
Expected: Pressing `.`, `n`, `?` opens the correct modal/screen. Help screen renders. Action menu renders.

- [ ] **Step 4: Commit**

```bash
git add torchard-rs/
git commit -m "add SessionListScreen actions and stub screens"
```

---

## Task 12: Simple input screens — Rename, EditBranch, Settings

**Files:**
- Create: `torchard-rs/src/tui/rename.rs`
- Create: `torchard-rs/src/tui/edit_branch.rs`
- Create: `torchard-rs/src/tui/settings.rs`

These are similar: centered box, text input, enter to confirm, escape to cancel.

- [ ] **Step 1: Write rename.rs**

Port `RenameSessionScreen` and `RenameWindowScreen` from `torchard/tui/views/rename_session.py`. Both are simple text inputs. RenameSession calls `manager.rename_session()`. RenameWindow calls `tmux::rename_window()`. Both pop on success.

Implement a shared text input widget pattern: `input: String`, `cursor_pos: usize`, handle left/right/backspace/delete/home/end keys.

- [ ] **Step 2: Write edit_branch.rs**

Port `EditBranchScreen` from `torchard/tui/views/edit_branch.py`. Filterable branch list + custom branch input. Calls `manager.set_base_branch()`. Pops on success.

- [ ] **Step 3: Write settings.rs**

Port `SettingsScreen` from `torchard/tui/views/settings.py`. Shows config keys with editable inputs. Tab between fields. Enter saves all. Escape cancels.

- [ ] **Step 4: Wire up in Screen enum and verify**

Run: `cd torchard-rs && cargo build`

- [ ] **Step 5: Commit**

```bash
git add torchard-rs/
git commit -m "add Rename, EditBranch, and Settings screens"
```

---

## Task 13: NewSessionScreen (multi-step wizard)

**Files:**
- Create: `torchard-rs/src/tui/new_session.rs`

- [ ] **Step 1: Write new_session.rs**

Port `NewSessionScreen` from `torchard/tui/views/new_session.py`. The most complex screen after SessionList.

Key behaviors to port:
- `WizardStep` enum with `PickRepo`, `PickBranch`, `PickSubsystem` variants
- `PickRepo`: scan `repos_dir` for directories, filter by name/path, "+ Add new repo path..." option, path entry sub-mode
- `PickBranch`: `git::list_branches()`, filter, "+ New branch: ..." option
- `PickSubsystem`: `manager::detect_subsystems()`, "/ (root)" option
- Auto-naming: `_auto_session_name()` logic — repo name for default branch, branch name otherwise, deduplicate with `-2`, `-3`, etc.
- After branch selection: auto-name and skip to subsystem or create directly
- Escape: go back one step (subsystem → branch, branch → repo, repo → pop)
- On create: `manager.create_session()`, `tmux::switch_client()`, exit app

Each step renders: title with step number, hint text, filter input, list items.

- [ ] **Step 2: Manual test**

Run the app, press `n`, select "New session", walk through the wizard.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add NewSessionScreen wizard"
```

---

## Task 14: NewTabScreen and AdoptSessionScreen

**Files:**
- Create: `torchard-rs/src/tui/new_tab.rs`
- Create: `torchard-rs/src/tui/adopt_session.rs`

- [ ] **Step 1: Write new_tab.rs**

Port `NewTabScreen` from `torchard/tui/views/new_tab.py`. Simple text input for branch name. On submit: `manager.add_tab()`, then `tmux::send_keys()` to send "claude" + Enter, then `write_switch` and exit.

- [ ] **Step 2: Write adopt_session.rs**

Port `AdoptSessionScreen` from `torchard/tui/views/adopt_session.py`. Two-step wizard: pick repo (from known repos, plus add-by-path), pick branch. On complete: `manager.adopt_session()`, pop.

Similar to NewSessionScreen's PickRepo/PickBranch steps but simpler (uses known repos, not directory scan).

- [ ] **Step 3: Wire up and verify**

Run: `cd torchard-rs && cargo build`

- [ ] **Step 4: Commit**

```bash
git add torchard-rs/
git commit -m "add NewTab and AdoptSession screens"
```

---

## Task 15: ReviewScreen (with background checkout)

**Files:**
- Create: `torchard-rs/src/tui/review.rs`

- [ ] **Step 1: Write review.rs**

Port `ReviewScreen` from `torchard/tui/views/review.py`. Text input for PR number or branch name. Tab cycles repos (sorted by most recently active session).

On submit: shows "Checking out..." status. Spawns background thread that runs `manager.checkout_and_review()` (which calls git fetch — can take seconds). The thread sends a `BackgroundResult::CheckoutComplete` via the `bg_tx` channel.

`on_background_result()` handles the result: on success, write switch file and return `ScreenAction::Quit`. On error, display error message inline.

Note on Connection threading: The background thread cannot use the Manager directly (Connection is !Send). Instead, extract the needed data (repo path, PR/branch string) and call `git::fetch_branch()` + `git::create_worktree()` directly in the thread, then send the result back. The main thread handles DB writes (add_session, add_worktree) after the thread completes.

- [ ] **Step 2: Manual test**

Run the app, press `r`, enter a branch name.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add ReviewScreen with background checkout"
```

---

## Task 16: HistoryScreen

**Files:**
- Create: `torchard-rs/src/tui/history.rs`

- [ ] **Step 1: Write history.rs**

Port `HistoryScreen` from `torchard/tui/views/history.py`. Uses `conversation_index::parse_index()`.

Key behaviors:
- 4-column table: Date, Project (truncated, `~/` prefix), Branch (truncated), Summary (truncated)
- Scoping: `scope_paths` and `scope_label`, toggle with `t`
- Sorting: 4 modes (date/project/branch/summary) toggled by `d`/`p`/`b`/`s`. Same key toggles direction. Date defaults descending, others ascending.
- Filtering: substring match (not fuzzy) against all 4 fields
- Resume (enter): resolve session ID, find matching managed session, create tmux window with `claude --resume <uuid>`, write switch, exit app
- Title bar shows scope, count, current sort + direction

- [ ] **Step 2: Manual test**

Run the app, press `h` on a session.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add HistoryScreen"
```

---

## Task 17: CleanupScreen (with background staleness check)

**Files:**
- Create: `torchard-rs/src/tui/cleanup.rs`

- [ ] **Step 1: Write cleanup.rs**

Port `CleanupScreen` from `torchard/tui/views/cleanup.py`.

Key behaviors:
- Shows all worktrees in 5-column table: checkbox, branch, session name, path (truncate_start), status
- On mount: render table with "checking..." status, spawn background thread
- Background thread: take owned copies of worktree list + repo list, call `git::is_branch_merged()` and `git::has_remote_branch()` for each, send `BackgroundResult::StaleWorktrees` back
- `on_background_result()`: update status cells to "ok" or "stale"
- Selection: `HashSet<String>`, space/enter toggle, a select all, A deselect all
- Delete (d): push ConfirmModal, on confirm bulk-delete via `manager.cleanup_worktree()`
- Status bar: total, stale count, selected count

- [ ] **Step 2: Manual test**

Run the app, press `c`.

- [ ] **Step 3: Commit**

```bash
git add torchard-rs/
git commit -m "add CleanupScreen with background staleness check"
```

---

## Task 18: Full integration test and polish

- [ ] **Step 1: Run all tests**

Run: `cd torchard-rs && cargo test`
Expected: All unit tests pass.

- [ ] **Step 2: Side-by-side manual testing**

Test every interaction against the Python version:
- Session list renders identically (colors, indicators, sort order)
- Create session wizard works end-to-end
- Expand/collapse shows correct window data
- Filter works with fuzzy matching
- All action menu items work
- Delete session/tab with confirmation
- Review checkout works
- History browsing and resume works
- Cleanup staleness detection and deletion works
- Settings save correctly
- Help screen shows correct content

- [ ] **Step 3: Build release binary and test startup time**

Run: `cd torchard-rs && cargo build --release`

Compare: `time torchard` vs `time ./target/release/torchard-rs`

Expected: Rust binary starts in <50ms vs Python's 200-500ms.

- [ ] **Step 4: Final commit**

```bash
git add torchard-rs/
git commit -m "polish and verify torchard-rs parity with Python version"
```
