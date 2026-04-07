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
    let now = utc_now_iso();
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

fn utc_now_iso() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339).unwrap()
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
        let tmp = std::env::temp_dir().join(format!(
            "torchard-test-init-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        let conn = init_db(&tmp);
        let repos = get_repos(&conn);
        assert!(repos.is_empty());
        assert!(get_config(&conn, "repos_dir").is_some());
        assert!(get_config(&conn, "worktrees_dir").is_some());
        let _ = std::fs::remove_file(&tmp);
    }
}
