"""Tests for Manager orchestration layer."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import patch, MagicMock

import pytest

from torchard.core.db import get_repos, get_sessions, get_worktrees, init_db
from torchard.core.manager import Manager
from torchard.core.models import Repo, Session, Worktree


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def conn():
    c = init_db(":memory:")
    yield c
    c.close()


@pytest.fixture
def mgr(conn):
    return Manager(conn)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

REPO_PATH = "/repos/myrepo"
REPO_NAME = "myrepo"
DEFAULT_BRANCH = "main"
SESSION_NAME = "myrepo-session"


def _patch_git_detect(branch="main"):
    return patch("torchard.core.manager.git.detect_default_branch", return_value=branch)


def _patch_tmux_new_session():
    return patch("torchard.core.manager.tmux.new_session")


def _patch_tmux_new_window():
    return patch("torchard.core.manager.tmux.new_window")


def _patch_tmux_kill_session():
    return patch("torchard.core.manager.tmux.kill_session")


def _patch_git_create_worktree():
    return patch("torchard.core.manager.git.create_worktree")


def _patch_git_fetch_and_pull():
    return patch("torchard.core.manager.git.fetch_and_pull")


def _patch_git_remove_worktree():
    return patch("torchard.core.manager.git.remove_worktree")


# ---------------------------------------------------------------------------
# create_session
# ---------------------------------------------------------------------------

class TestCreateSession:
    def test_creates_repo_and_session(self, mgr, conn):
        with _patch_git_detect("main") as mock_detect, _patch_tmux_new_session() as mock_tmux:
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)

        assert session.id is not None
        assert session.name == SESSION_NAME
        assert session.base_branch == "main"

        repos = get_repos(conn)
        assert len(repos) == 1
        assert repos[0].path == REPO_PATH
        assert repos[0].default_branch == "main"

        mock_detect.assert_called_once_with(REPO_PATH)
        mock_tmux.assert_called_once_with(SESSION_NAME, REPO_PATH)

    def test_reuses_existing_repo(self, mgr, conn):
        with _patch_git_detect("main"), _patch_tmux_new_session():
            mgr.create_session(REPO_PATH, "main", SESSION_NAME)

        with _patch_git_detect("main") as mock_detect, _patch_tmux_new_session():
            mgr.create_session(REPO_PATH, "main", "second-session")

        # detect_default_branch should NOT be called the second time
        mock_detect.assert_not_called()
        # Still only one repo
        assert len(get_repos(conn)) == 1
        assert len(get_sessions(conn)) == 2

    def test_returns_session_object(self, mgr):
        with (
            _patch_git_detect(),
            _patch_tmux_new_session(),
            _patch_tmux_new_window(),
            _patch_git_create_worktree(),
            _patch_git_fetch_and_pull(),
            patch("torchard.core.manager.subprocess.run"),
        ):
            session = mgr.create_session(REPO_PATH, "feature", SESSION_NAME)
        assert isinstance(session, Session)
        assert session.base_branch == "feature"

    def test_uses_repo_path_basename_as_name(self, mgr, conn):
        with _patch_git_detect(), _patch_tmux_new_session():
            mgr.create_session("/some/path/cool-project", "main", "s1")
        repos = get_repos(conn)
        assert repos[0].name == "cool-project"


# ---------------------------------------------------------------------------
# add_tab
# ---------------------------------------------------------------------------

class TestAddTab:
    def _setup_session(self, mgr):
        with _patch_git_detect("main"), _patch_tmux_new_session():
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)
        return session

    def test_creates_worktree_and_window(self, mgr, conn):
        session = self._setup_session(mgr)

        with _patch_git_create_worktree() as mock_git, _patch_tmux_new_window() as mock_window:
            wt = mgr.add_tab(session.id, "feature-x")

        expected_path = str(Path.home() / "dev" / "worktrees" / REPO_NAME / "feature-x")
        mock_git.assert_called_once_with(REPO_PATH, expected_path, "feature-x", "main")
        mock_window.assert_called_once_with(SESSION_NAME, "feature-x", expected_path)

        assert wt.id is not None
        assert wt.branch == "feature-x"
        assert wt.path == expected_path
        assert wt.session_id == session.id

    def test_worktree_recorded_in_db(self, mgr, conn):
        session = self._setup_session(mgr)

        with _patch_git_create_worktree(), _patch_tmux_new_window():
            mgr.add_tab(session.id, "my-branch")

        wts = get_worktrees(conn)
        assert len(wts) == 1
        assert wts[0].branch == "my-branch"

    def test_raises_for_missing_session(self, mgr):
        with pytest.raises(ValueError, match="Session 999"):
            mgr.add_tab(999, "branch")

    def test_worktree_path_uses_repo_name(self, mgr):
        with _patch_git_detect(), _patch_tmux_new_session():
            session = mgr.create_session("/repos/awesome-repo", "main", "s1")

        with _patch_git_create_worktree() as mock_git, _patch_tmux_new_window():
            mgr.add_tab(session.id, "feat")

        expected_path = str(Path.home() / "dev" / "worktrees" / "awesome-repo" / "feat")
        call_args = mock_git.call_args
        assert call_args[0][1] == expected_path


# ---------------------------------------------------------------------------
# delete_session
# ---------------------------------------------------------------------------

class TestDeleteSession:
    def _setup_session_with_worktree(self, mgr):
        with _patch_git_detect("main"), _patch_tmux_new_session():
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)
        with _patch_git_create_worktree(), _patch_tmux_new_window():
            wt = mgr.add_tab(session.id, "feat")
        return session, wt

    def test_kills_tmux_session_and_removes_db(self, mgr, conn):
        with _patch_git_detect(), _patch_tmux_new_session():
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)

        with _patch_tmux_kill_session() as mock_kill:
            mgr.delete_session(session.id)

        mock_kill.assert_called_once_with(SESSION_NAME)
        assert get_sessions(conn) == []

    def test_cleanup_worktrees_false_leaves_worktrees(self, mgr, conn):
        session, wt = self._setup_session_with_worktree(mgr)

        with _patch_tmux_kill_session():
            mgr.delete_session(session.id, cleanup_worktrees=False)

        # Session removed, but worktrees remain
        assert get_sessions(conn) == []
        assert len(get_worktrees(conn)) == 1

    def test_cleanup_worktrees_true_removes_all(self, mgr, conn):
        session, wt = self._setup_session_with_worktree(mgr)

        with _patch_tmux_kill_session(), _patch_git_remove_worktree() as mock_rm:
            mgr.delete_session(session.id, cleanup_worktrees=True)

        mock_rm.assert_called_once_with(REPO_PATH, wt.path)
        assert get_sessions(conn) == []
        assert get_worktrees(conn) == []

    def test_tmux_error_silently_continues(self, mgr, conn):
        """delete_session should still clean up DB even if tmux kill fails."""
        with _patch_git_detect(), _patch_tmux_new_session():
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)

        with patch("torchard.core.manager.tmux.kill_session", side_effect=Exception("tmux gone")):
            # Should not raise
            mgr.delete_session(session.id)

        assert get_sessions(conn) == []

    def test_raises_for_missing_session(self, mgr):
        with pytest.raises(ValueError, match="Session 999"):
            mgr.delete_session(999)


# ---------------------------------------------------------------------------
# cleanup_worktree
# ---------------------------------------------------------------------------

class TestCleanupWorktree:
    def test_removes_git_worktree_and_db_record(self, mgr, conn):
        with _patch_git_detect(), _patch_tmux_new_session():
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)
        with _patch_git_create_worktree(), _patch_tmux_new_window():
            wt = mgr.add_tab(session.id, "feat")

        with _patch_git_remove_worktree() as mock_rm:
            mgr.cleanup_worktree(wt.id)

        mock_rm.assert_called_once_with(REPO_PATH, wt.path)
        assert get_worktrees(conn) == []

    def test_raises_for_missing_worktree(self, mgr):
        with pytest.raises(ValueError, match="Worktree 999"):
            mgr.cleanup_worktree(999)


# ---------------------------------------------------------------------------
# get_stale_worktrees
# ---------------------------------------------------------------------------

class TestGetStaleWorktrees:
    def _create_worktree_record(self, mgr):
        with _patch_git_detect("main"), _patch_tmux_new_session():
            session = mgr.create_session(REPO_PATH, "main", SESSION_NAME)
        with _patch_git_create_worktree(), _patch_tmux_new_window():
            wt = mgr.add_tab(session.id, "feat")
        return wt

    def test_merged_branch_is_stale(self, mgr):
        wt = self._create_worktree_record(mgr)

        with patch("torchard.core.manager.git.is_branch_merged", return_value=True), \
             patch("torchard.core.manager.git.has_remote_branch", return_value=True):
            stale = mgr.get_stale_worktrees()

        assert len(stale) == 1
        assert stale[0].id == wt.id

    def test_deleted_remote_branch_is_stale(self, mgr):
        wt = self._create_worktree_record(mgr)

        with patch("torchard.core.manager.git.is_branch_merged", return_value=False), \
             patch("torchard.core.manager.git.has_remote_branch", return_value=False):
            stale = mgr.get_stale_worktrees()

        assert len(stale) == 1

    def test_active_branch_not_stale(self, mgr):
        self._create_worktree_record(mgr)

        with patch("torchard.core.manager.git.is_branch_merged", return_value=False), \
             patch("torchard.core.manager.git.has_remote_branch", return_value=True):
            stale = mgr.get_stale_worktrees()

        assert stale == []

    def test_empty_when_no_worktrees(self, mgr):
        with patch("torchard.core.manager.git.is_branch_merged", return_value=True), \
             patch("torchard.core.manager.git.has_remote_branch", return_value=False):
            stale = mgr.get_stale_worktrees()
        assert stale == []


# ---------------------------------------------------------------------------
# list_sessions
# ---------------------------------------------------------------------------

class TestListSessions:
    def test_enriches_with_live_tmux_state(self, mgr):
        with _patch_git_detect(), _patch_tmux_new_session():
            mgr.create_session(REPO_PATH, "main", SESSION_NAME)

        live = [{"name": SESSION_NAME, "windows": 3, "attached": True}]
        with patch("torchard.core.manager.tmux.list_sessions", return_value=live):
            result = mgr.list_sessions()

        assert len(result) == 1
        assert result[0]["name"] == SESSION_NAME
        assert result[0]["windows"] == 3
        assert result[0]["attached"] is True
        assert result[0]["live"] is True

    def test_session_not_in_tmux(self, mgr):
        with _patch_git_detect(), _patch_tmux_new_session():
            mgr.create_session(REPO_PATH, "main", SESSION_NAME)

        with patch("torchard.core.manager.tmux.list_sessions", return_value=[]):
            result = mgr.list_sessions()

        assert len(result) == 1
        assert result[0]["live"] is False
        assert result[0]["windows"] is None
        assert result[0]["attached"] is False

    def test_empty_when_no_db_sessions(self, mgr):
        with patch("torchard.core.manager.tmux.list_sessions", return_value=[]):
            result = mgr.list_sessions()
        assert result == []

    def test_multiple_sessions(self, mgr):
        with _patch_git_detect(), _patch_tmux_new_session():
            mgr.create_session(REPO_PATH, "main", "session-a")
            mgr.create_session(REPO_PATH, "main", "session-b")

        live = [
            {"name": "session-a", "windows": 2, "attached": False},
        ]
        with patch("torchard.core.manager.tmux.list_sessions", return_value=live):
            result = mgr.list_sessions()

        assert len(result) == 2
        by_name = {r["name"]: r for r in result}
        assert by_name["session-a"]["live"] is True
        assert by_name["session-b"]["live"] is False


# ---------------------------------------------------------------------------
# scan_existing
# ---------------------------------------------------------------------------

class TestScanExisting:
    def test_scan_no_dirs(self, mgr, conn, tmp_path):
        """scan_existing with empty directories does nothing."""
        home_dev = tmp_path / "dev"
        home_dev.mkdir()
        worktrees_root = home_dev / "worktrees"
        worktrees_root.mkdir()

        with patch("torchard.core.manager.Path.home", return_value=tmp_path), \
             patch("torchard.core.manager.tmux.list_sessions", return_value=[]):
            mgr.scan_existing()

        assert get_repos(conn) == []

    def test_scan_discovers_git_repo(self, mgr, conn, tmp_path):
        home_dev = tmp_path / "dev"
        home_dev.mkdir()
        (home_dev / "worktrees").mkdir()
        repo_dir = home_dev / "coolrepo"
        repo_dir.mkdir()
        (repo_dir / ".git").mkdir()

        with patch("torchard.core.manager.Path.home", return_value=tmp_path), \
             patch("torchard.core.manager.git.detect_default_branch", return_value="main"), \
             patch("torchard.core.manager.tmux.list_sessions", return_value=[]):
            mgr.scan_existing()

        repos = get_repos(conn)
        assert len(repos) == 1
        assert repos[0].name == "coolrepo"

    def test_scan_does_not_duplicate_known_repo(self, mgr, conn, tmp_path):
        home_dev = tmp_path / "dev"
        home_dev.mkdir()
        (home_dev / "worktrees").mkdir()
        repo_dir = home_dev / "coolrepo"
        repo_dir.mkdir()
        (repo_dir / ".git").mkdir()

        # Pre-populate the repo
        with patch("torchard.core.manager.git.detect_default_branch", return_value="main"), \
             _patch_tmux_new_session():
            mgr.create_session(str(repo_dir), "main", "my-session")

        with patch("torchard.core.manager.Path.home", return_value=tmp_path), \
             patch("torchard.core.manager.git.detect_default_branch", return_value="main") as mock_detect, \
             patch("torchard.core.manager.tmux.list_sessions", return_value=[]):
            mgr.scan_existing()

        # detect_default_branch should NOT be called again for the already-known repo
        mock_detect.assert_not_called()
        assert len(get_repos(conn)) == 1

    def test_scan_discovers_worktrees(self, mgr, conn, tmp_path):
        home_dev = tmp_path / "dev"
        home_dev.mkdir()
        repo_dir = home_dev / "myrepo"
        repo_dir.mkdir()
        (repo_dir / ".git").mkdir()
        wt_dir = home_dev / "worktrees" / "myrepo" / "feature-branch"
        wt_dir.mkdir(parents=True)

        with patch("torchard.core.manager.Path.home", return_value=tmp_path), \
             patch("torchard.core.manager.git.detect_default_branch", return_value="main"), \
             patch("torchard.core.manager.tmux.list_sessions", return_value=[]):
            mgr.scan_existing()

        wts = get_worktrees(conn)
        assert len(wts) == 1
        assert wts[0].branch == "feature-branch"

    def test_scan_matches_tmux_session_to_repo(self, mgr, conn, tmp_path):
        home_dev = tmp_path / "dev"
        home_dev.mkdir()
        (home_dev / "worktrees").mkdir()
        repo_dir = home_dev / "myrepo"
        repo_dir.mkdir()
        (repo_dir / ".git").mkdir()

        live_sessions = [{"name": "myrepo-work", "windows": 1, "attached": False}]

        with patch("torchard.core.manager.Path.home", return_value=tmp_path), \
             patch("torchard.core.manager.git.detect_default_branch", return_value="main"), \
             patch("torchard.core.manager.tmux.list_sessions", return_value=live_sessions):
            mgr.scan_existing()

        sessions = get_sessions(conn)
        assert len(sessions) == 1
        assert sessions[0].name == "myrepo-work"
