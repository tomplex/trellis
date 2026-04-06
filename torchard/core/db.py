import sqlite3
from pathlib import Path

from torchard.core.models import Repo, Session, Worktree

_DEFAULT_DB_PATH = Path.home() / ".local" / "share" / "torchard" / "torchard.db"

_CREATE_REPOS = """
CREATE TABLE IF NOT EXISTS repos (
    id             INTEGER PRIMARY KEY,
    path           TEXT NOT NULL,
    name           TEXT NOT NULL,
    default_branch TEXT NOT NULL
)
"""

_CREATE_SESSIONS = """
CREATE TABLE IF NOT EXISTS sessions (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    repo_id     INTEGER NOT NULL,
    base_branch TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    FOREIGN KEY (repo_id) REFERENCES repos(id)
)
"""

_CREATE_WORKTREES = """
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
)
"""


def _connect(db_path: Path | str) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def init_db(db_path: Path | str | None = None) -> sqlite3.Connection:
    if db_path is None:
        db_path = _DEFAULT_DB_PATH
        db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = _connect(db_path)
    conn.execute(_CREATE_REPOS)
    conn.execute(_CREATE_SESSIONS)
    conn.execute(_CREATE_WORKTREES)
    conn.commit()
    return conn


# --- Repos ---

def add_repo(conn: sqlite3.Connection, repo: Repo) -> Repo:
    cur = conn.execute(
        "INSERT INTO repos (path, name, default_branch) VALUES (?, ?, ?)",
        (repo.path, repo.name, repo.default_branch),
    )
    conn.commit()
    return Repo(id=cur.lastrowid, path=repo.path, name=repo.name, default_branch=repo.default_branch)


def get_repos(conn: sqlite3.Connection) -> list[Repo]:
    rows = conn.execute("SELECT id, path, name, default_branch FROM repos").fetchall()
    return [Repo(id=r["id"], path=r["path"], name=r["name"], default_branch=r["default_branch"]) for r in rows]


# --- Sessions ---

def add_session(conn: sqlite3.Connection, session: Session) -> Session:
    cur = conn.execute(
        "INSERT INTO sessions (name, repo_id, base_branch, created_at) VALUES (?, ?, ?, ?)",
        (session.name, session.repo_id, session.base_branch, session.created_at),
    )
    conn.commit()
    return Session(
        id=cur.lastrowid,
        name=session.name,
        repo_id=session.repo_id,
        base_branch=session.base_branch,
        created_at=session.created_at,
    )


def get_sessions(conn: sqlite3.Connection) -> list[Session]:
    rows = conn.execute("SELECT id, name, repo_id, base_branch, created_at FROM sessions").fetchall()
    return [
        Session(id=r["id"], name=r["name"], repo_id=r["repo_id"], base_branch=r["base_branch"], created_at=r["created_at"])
        for r in rows
    ]


def get_session_by_name(conn: sqlite3.Connection, name: str) -> Session | None:
    row = conn.execute(
        "SELECT id, name, repo_id, base_branch, created_at FROM sessions WHERE name = ?", (name,)
    ).fetchone()
    if row is None:
        return None
    return Session(id=row["id"], name=row["name"], repo_id=row["repo_id"], base_branch=row["base_branch"], created_at=row["created_at"])


def delete_session(conn: sqlite3.Connection, session_id: int) -> None:
    conn.execute("DELETE FROM sessions WHERE id = ?", (session_id,))
    conn.commit()


# --- Worktrees ---

def add_worktree(conn: sqlite3.Connection, worktree: Worktree) -> Worktree:
    cur = conn.execute(
        "INSERT INTO worktrees (session_id, repo_id, path, branch, tmux_window, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        (worktree.session_id, worktree.repo_id, worktree.path, worktree.branch, worktree.tmux_window, worktree.created_at),
    )
    conn.commit()
    return Worktree(
        id=cur.lastrowid,
        session_id=worktree.session_id,
        repo_id=worktree.repo_id,
        path=worktree.path,
        branch=worktree.branch,
        tmux_window=worktree.tmux_window,
        created_at=worktree.created_at,
    )


def get_worktrees(conn: sqlite3.Connection) -> list[Worktree]:
    rows = conn.execute(
        "SELECT id, session_id, repo_id, path, branch, tmux_window, created_at FROM worktrees"
    ).fetchall()
    return [
        Worktree(
            id=r["id"], session_id=r["session_id"], repo_id=r["repo_id"],
            path=r["path"], branch=r["branch"], tmux_window=r["tmux_window"], created_at=r["created_at"],
        )
        for r in rows
    ]


def get_worktrees_for_session(conn: sqlite3.Connection, session_id: int) -> list[Worktree]:
    rows = conn.execute(
        "SELECT id, session_id, repo_id, path, branch, tmux_window, created_at FROM worktrees WHERE session_id = ?",
        (session_id,),
    ).fetchall()
    return [
        Worktree(
            id=r["id"], session_id=r["session_id"], repo_id=r["repo_id"],
            path=r["path"], branch=r["branch"], tmux_window=r["tmux_window"], created_at=r["created_at"],
        )
        for r in rows
    ]


def delete_worktree(conn: sqlite3.Connection, worktree_id: int) -> None:
    conn.execute("DELETE FROM worktrees WHERE id = ?", (worktree_id,))
    conn.commit()
