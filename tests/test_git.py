"""Tests for torchard.core.git — all subprocess calls are mocked."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest

from torchard.core.git import (
    GitError,
    create_worktree,
    detect_default_branch,
    has_remote_branch,
    is_branch_merged,
    list_branches,
    list_worktrees,
    remove_worktree,
)

REPO = "/home/user/myrepo"


def _completed(returncode: int = 0, stdout: str = "", stderr: str = "") -> MagicMock:
    m = MagicMock()
    m.returncode = returncode
    m.stdout = stdout
    m.stderr = stderr
    return m


class TestDetectDefaultBranch:
    def test_uses_remote_head_main(self):
        with patch("subprocess.run", return_value=_completed(stdout="refs/remotes/origin/main\n")) as mock_run:
            branch = detect_default_branch(REPO)

        assert branch == "main"
        mock_run.assert_called_once_with(
            ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )

    def test_uses_remote_head_master(self):
        with patch("subprocess.run", return_value=_completed(stdout="refs/remotes/origin/master\n")):
            branch = detect_default_branch(REPO)
        assert branch == "master"

    def test_falls_back_to_local_main(self):
        responses = [
            _completed(returncode=1),  # symbolic-ref fails
            _completed(stdout="main\nfeature-x\n"),  # local branches
        ]
        with patch("subprocess.run", side_effect=responses):
            branch = detect_default_branch(REPO)
        assert branch == "main"

    def test_falls_back_to_local_master(self):
        responses = [
            _completed(returncode=1),
            _completed(stdout="master\nfeature-x\n"),
        ]
        with patch("subprocess.run", side_effect=responses):
            branch = detect_default_branch(REPO)
        assert branch == "master"

    def test_raises_when_cannot_determine(self):
        responses = [
            _completed(returncode=1),
            _completed(stdout="feature-a\nfeature-b\n"),
        ]
        with patch("subprocess.run", side_effect=responses):
            with pytest.raises(GitError, match="Could not determine default branch"):
                detect_default_branch(REPO)

    def test_raises_on_branch_list_failure(self):
        responses = [
            _completed(returncode=1),
            _completed(returncode=1, stderr="not a git repo"),
        ]
        with patch("subprocess.run", side_effect=responses):
            with pytest.raises(GitError, match="Failed to list branches"):
                detect_default_branch(REPO)


class TestListBranches:
    def test_returns_branch_names(self):
        with patch("subprocess.run", return_value=_completed(stdout="main\nfeature-x\nbugfix\n")) as mock_run:
            branches = list_branches(REPO)

        mock_run.assert_called_once_with(
            ["git", "branch", "--format=%(refname:short)"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )
        assert branches == ["main", "feature-x", "bugfix"]

    def test_returns_empty_list_for_no_branches(self):
        with patch("subprocess.run", return_value=_completed(stdout="")):
            assert list_branches(REPO) == []

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=128, stderr="not a repo")):
            with pytest.raises(GitError, match="Failed to list branches"):
                list_branches(REPO)


class TestListWorktrees:
    PORCELAIN = (
        "worktree /home/user/myrepo\n"
        "HEAD abc123\n"
        "branch refs/heads/main\n"
        "\n"
        "worktree /home/user/myrepo-feat\n"
        "HEAD def456\n"
        "branch refs/heads/feature-x\n"
        "\n"
    )

    def test_parses_worktrees(self):
        with patch("subprocess.run", return_value=_completed(stdout=self.PORCELAIN)) as mock_run:
            result = list_worktrees(REPO)

        mock_run.assert_called_once_with(
            ["git", "worktree", "list", "--porcelain"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )
        assert result == [
            {"path": "/home/user/myrepo", "branch": "main", "commit": "abc123"},
            {"path": "/home/user/myrepo-feat", "branch": "feature-x", "commit": "def456"},
        ]

    def test_handles_detached_head(self):
        porcelain = (
            "worktree /home/user/myrepo\n"
            "HEAD abc123\n"
            "detached\n"
        )
        with patch("subprocess.run", return_value=_completed(stdout=porcelain)):
            result = list_worktrees(REPO)
        assert result[0]["branch"] == "(detached)"

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="not a repo")):
            with pytest.raises(GitError, match="Failed to list worktrees"):
                list_worktrees(REPO)


class TestCreateWorktree:
    def test_creates_worktree(self):
        # list_worktrees returns empty (no conflicts), then add succeeds
        responses = [
            _completed(stdout="worktree /home/user/myrepo\nHEAD abc\nbranch refs/heads/main\n"),
            _completed(),
        ]
        with patch("subprocess.run", side_effect=responses) as mock_run:
            create_worktree(REPO, "/home/user/myrepo-feat", "feature-x", "main")

        mock_run.assert_called_with(
            ["git", "worktree", "add", "-b", "feature-x", "/home/user/myrepo-feat", "main"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )

    def test_raises_when_path_already_in_use(self):
        existing_output = (
            "worktree /home/user/myrepo-feat\n"
            "HEAD abc\n"
            "branch refs/heads/feature-x\n"
        )
        with patch("subprocess.run", return_value=_completed(stdout=existing_output)):
            with pytest.raises(GitError, match="already in use"):
                create_worktree(REPO, "/home/user/myrepo-feat", "new-branch", "main")

    def test_raises_on_git_failure(self):
        responses = [
            _completed(stdout="worktree /home/user/myrepo\nHEAD abc\nbranch refs/heads/main\n"),
            _completed(returncode=1, stderr="branch already exists"),
        ]
        with patch("subprocess.run", side_effect=responses):
            with pytest.raises(GitError, match="Failed to create worktree"):
                create_worktree(REPO, "/tmp/new-wt", "existing-branch", "main")


class TestRemoveWorktree:
    def test_removes_worktree(self):
        with patch("subprocess.run", return_value=_completed()) as mock_run:
            remove_worktree(REPO, "/home/user/myrepo-feat")

        mock_run.assert_called_once_with(
            ["git", "worktree", "remove", "/home/user/myrepo-feat"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="not a worktree")):
            with pytest.raises(GitError, match="Failed to remove worktree"):
                remove_worktree(REPO, "/tmp/nonexistent")


class TestIsBranchMerged:
    def test_returns_true_when_merged(self):
        with patch("subprocess.run", return_value=_completed(stdout="main\nfeature-x\n")) as mock_run:
            result = is_branch_merged(REPO, "feature-x", "main")

        mock_run.assert_called_once_with(
            ["git", "branch", "--merged", "main", "--format=%(refname:short)"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )
        assert result is True

    def test_returns_false_when_not_merged(self):
        with patch("subprocess.run", return_value=_completed(stdout="main\n")):
            result = is_branch_merged(REPO, "feature-x", "main")
        assert result is False

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="bad ref")):
            with pytest.raises(GitError, match="Failed to check merged branches"):
                is_branch_merged(REPO, "branch", "main")


class TestHasRemoteBranch:
    def test_returns_true_when_remote_branch_exists(self):
        output = "abc123\trefs/heads/feature-x\n"
        with patch("subprocess.run", return_value=_completed(stdout=output)) as mock_run:
            result = has_remote_branch(REPO, "feature-x")

        mock_run.assert_called_once_with(
            ["git", "ls-remote", "--heads", "origin", "feature-x"],
            capture_output=True, text=True, check=False, cwd=REPO,
        )
        assert result is True

    def test_returns_false_when_remote_branch_missing(self):
        with patch("subprocess.run", return_value=_completed(stdout="")):
            result = has_remote_branch(REPO, "no-such-branch")
        assert result is False

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="could not read")):
            with pytest.raises(GitError, match="Failed to query remote"):
                has_remote_branch(REPO, "branch")
