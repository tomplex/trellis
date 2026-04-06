import sqlite3
import pytest

from torchard.core.db import (
    add_repo,
    add_session,
    add_worktree,
    delete_session,
    delete_worktree,
    get_repos,
    get_session_by_name,
    get_sessions,
    get_worktrees,
    get_worktrees_for_session,
    init_db,
)
from torchard.core.models import Repo, Session, Worktree


@pytest.fixture
def conn():
    """In-memory SQLite connection with schema initialized."""
    c = init_db(db_path=":memory:")
    yield c
    c.close()


# --- Helpers ---

def make_repo(conn, path="/repos/myrepo", name="myrepo", default_branch="main"):
    return add_repo(conn, Repo(path=path, name=name, default_branch=default_branch))


def make_session(conn, repo_id, name="my-session", base_branch="main"):
    return add_session(conn, Session(name=name, repo_id=repo_id, base_branch=base_branch, created_at="2026-01-01T00:00:00"))


def make_worktree(conn, repo_id, session_id=None, path="/worktrees/feat", branch="feat"):
    return add_worktree(conn, Worktree(
        repo_id=repo_id, session_id=session_id, path=path, branch=branch, created_at="2026-01-01T00:00:00"
    ))


# --- Repo tests ---

class TestRepos:
    def test_add_repo_returns_persisted(self, conn):
        repo = make_repo(conn)
        assert repo.id is not None
        assert repo.name == "myrepo"

    def test_get_repos_empty(self, conn):
        assert get_repos(conn) == []

    def test_get_repos_returns_all(self, conn):
        make_repo(conn, path="/repos/a", name="a")
        make_repo(conn, path="/repos/b", name="b")
        repos = get_repos(conn)
        assert len(repos) == 2
        assert {r.name for r in repos} == {"a", "b"}

    def test_repo_fields_roundtrip(self, conn):
        repo = make_repo(conn, path="/repos/x", name="x", default_branch="master")
        fetched = get_repos(conn)[0]
        assert fetched.path == "/repos/x"
        assert fetched.default_branch == "master"


# --- Session tests ---

class TestSessions:
    def test_add_session_returns_persisted(self, conn):
        repo = make_repo(conn)
        session = make_session(conn, repo.id)
        assert session.id is not None
        assert session.name == "my-session"

    def test_get_sessions_empty(self, conn):
        assert get_sessions(conn) == []

    def test_get_sessions_returns_all(self, conn):
        repo = make_repo(conn)
        make_session(conn, repo.id, name="s1")
        make_session(conn, repo.id, name="s2")
        sessions = get_sessions(conn)
        assert len(sessions) == 2
        assert {s.name for s in sessions} == {"s1", "s2"}

    def test_get_session_by_name_found(self, conn):
        repo = make_repo(conn)
        make_session(conn, repo.id, name="target")
        s = get_session_by_name(conn, "target")
        assert s is not None
        assert s.name == "target"
        assert s.repo_id == repo.id

    def test_get_session_by_name_not_found(self, conn):
        assert get_session_by_name(conn, "missing") is None

    def test_delete_session(self, conn):
        repo = make_repo(conn)
        s = make_session(conn, repo.id)
        delete_session(conn, s.id)
        assert get_sessions(conn) == []

    def test_session_fields_roundtrip(self, conn):
        repo = make_repo(conn)
        make_session(conn, repo.id, name="feat", base_branch="dev")
        s = get_sessions(conn)[0]
        assert s.base_branch == "dev"
        assert s.created_at == "2026-01-01T00:00:00"

    def test_session_foreign_key_enforced(self, conn):
        with pytest.raises(sqlite3.IntegrityError):
            add_session(conn, Session(name="bad", repo_id=999, base_branch="main", created_at="2026-01-01T00:00:00"))


# --- Worktree tests ---

class TestWorktrees:
    def test_add_worktree_returns_persisted(self, conn):
        repo = make_repo(conn)
        wt = make_worktree(conn, repo.id)
        assert wt.id is not None
        assert wt.branch == "feat"

    def test_get_worktrees_empty(self, conn):
        assert get_worktrees(conn) == []

    def test_get_worktrees_returns_all(self, conn):
        repo = make_repo(conn)
        make_worktree(conn, repo.id, path="/wt/a", branch="a")
        make_worktree(conn, repo.id, path="/wt/b", branch="b")
        wts = get_worktrees(conn)
        assert len(wts) == 2

    def test_worktree_nullable_session_id(self, conn):
        repo = make_repo(conn)
        wt = make_worktree(conn, repo.id, session_id=None)
        fetched = get_worktrees(conn)[0]
        assert fetched.session_id is None

    def test_worktree_with_session(self, conn):
        repo = make_repo(conn)
        session = make_session(conn, repo.id)
        wt = make_worktree(conn, repo.id, session_id=session.id)
        fetched = get_worktrees(conn)[0]
        assert fetched.session_id == session.id

    def test_get_worktrees_for_session(self, conn):
        repo = make_repo(conn)
        s1 = make_session(conn, repo.id, name="s1")
        s2 = make_session(conn, repo.id, name="s2")
        make_worktree(conn, repo.id, session_id=s1.id, path="/wt/1a", branch="1a")
        make_worktree(conn, repo.id, session_id=s1.id, path="/wt/1b", branch="1b")
        make_worktree(conn, repo.id, session_id=s2.id, path="/wt/2a", branch="2a")
        wts = get_worktrees_for_session(conn, s1.id)
        assert len(wts) == 2
        assert all(wt.session_id == s1.id for wt in wts)

    def test_get_worktrees_for_session_empty(self, conn):
        repo = make_repo(conn)
        s = make_session(conn, repo.id)
        assert get_worktrees_for_session(conn, s.id) == []

    def test_delete_worktree(self, conn):
        repo = make_repo(conn)
        wt = make_worktree(conn, repo.id)
        delete_worktree(conn, wt.id)
        assert get_worktrees(conn) == []

    def test_worktree_fields_roundtrip(self, conn):
        repo = make_repo(conn)
        add_worktree(conn, Worktree(
            repo_id=repo.id, session_id=None, path="/wt/feat",
            branch="feat-branch", tmux_window=3, created_at="2026-02-01T12:00:00"
        ))
        wt = get_worktrees(conn)[0]
        assert wt.path == "/wt/feat"
        assert wt.branch == "feat-branch"
        assert wt.tmux_window == 3
        assert wt.created_at == "2026-02-01T12:00:00"

    def test_worktree_foreign_key_enforced_repo(self, conn):
        with pytest.raises(sqlite3.IntegrityError):
            add_worktree(conn, Worktree(
                repo_id=999, session_id=None, path="/wt/x", branch="x", created_at="2026-01-01T00:00:00"
            ))

    def test_worktree_foreign_key_enforced_session(self, conn):
        repo = make_repo(conn)
        with pytest.raises(sqlite3.IntegrityError):
            add_worktree(conn, Worktree(
                repo_id=repo.id, session_id=999, path="/wt/x", branch="x", created_at="2026-01-01T00:00:00"
            ))
