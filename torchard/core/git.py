"""Thin wrapper around git and git-worktree CLI commands."""

from __future__ import annotations

import subprocess


class GitError(Exception):
    """Raised when a git command fails."""


def _run(args: list[str], cwd: str | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        args,
        capture_output=True,
        text=True,
        check=False,
        cwd=cwd,
    )


def detect_default_branch(repo_path: str) -> str:
    """Return the default branch name ("main" or "master").

    Inspects the remote HEAD first; falls back to checking local branch names.
    """
    # Try remote HEAD (e.g. "ref: refs/remotes/origin/HEAD -> origin/main")
    result = _run(
        ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
        cwd=repo_path,
    )
    if result.returncode == 0:
        ref = result.stdout.strip()
        # ref looks like "refs/remotes/origin/main"
        return ref.split("/")[-1]

    # Fall back to local branches: prefer "main", then "master"
    result = _run(["git", "branch", "--format=%(refname:short)"], cwd=repo_path)
    if result.returncode != 0:
        raise GitError(f"Failed to list branches in '{repo_path}': {result.stderr.strip()}")
    branches = result.stdout.strip().splitlines()
    for candidate in ("main", "master"):
        if candidate in branches:
            return candidate

    raise GitError(
        f"Could not determine default branch in '{repo_path}'; "
        f"found branches: {branches}"
    )


def list_branches(repo_path: str) -> list[str]:
    """Return a list of local branch names."""
    result = _run(
        ["git", "branch", "--format=%(refname:short)"],
        cwd=repo_path,
    )
    if result.returncode != 0:
        raise GitError(f"Failed to list branches in '{repo_path}': {result.stderr.strip()}")
    return [b for b in result.stdout.strip().splitlines() if b]


def list_worktrees(repo_path: str) -> list[dict]:
    """Return a list of worktrees, each with path, branch, and commit."""
    result = _run(
        ["git", "worktree", "list", "--porcelain"],
        cwd=repo_path,
    )
    if result.returncode != 0:
        raise GitError(f"Failed to list worktrees in '{repo_path}': {result.stderr.strip()}")

    worktrees = []
    current: dict = {}
    for line in result.stdout.splitlines():
        if line.startswith("worktree "):
            if current:
                worktrees.append(current)
            current = {"path": line[len("worktree "):], "branch": None, "commit": None}
        elif line.startswith("HEAD "):
            current["commit"] = line[len("HEAD "):]
        elif line.startswith("branch "):
            ref = line[len("branch "):]
            # refs/heads/main -> main
            current["branch"] = ref.removeprefix("refs/heads/")
        elif line == "detached":
            current["branch"] = "(detached)"
    if current:
        worktrees.append(current)
    return worktrees


def create_worktree(
    repo_path: str,
    worktree_path: str,
    branch: str,
    base_branch: str,
) -> None:
    """Create a new branch from base_branch and check it out at worktree_path.

    Raises GitError if the worktree path already exists or the branch already exists.
    """
    # Check if the worktree path is already in use
    existing = list_worktrees(repo_path)
    for wt in existing:
        if wt["path"] == worktree_path:
            raise GitError(f"Worktree path '{worktree_path}' is already in use")

    result = _run(
        ["git", "worktree", "add", "-b", branch, worktree_path, base_branch],
        cwd=repo_path,
    )
    if result.returncode != 0:
        stderr = result.stderr.strip()
        raise GitError(
            f"Failed to create worktree at '{worktree_path}' "
            f"(branch '{branch}' from '{base_branch}'): {stderr}"
        )


def remove_worktree(repo_path: str, worktree_path: str) -> None:
    """Remove a worktree."""
    result = _run(
        ["git", "worktree", "remove", worktree_path],
        cwd=repo_path,
    )
    if result.returncode != 0:
        raise GitError(
            f"Failed to remove worktree '{worktree_path}': {result.stderr.strip()}"
        )


def get_pr_branch(repo_path: str, pr_number: int) -> str:
    """Get the head branch name for a PR number via gh CLI."""
    result = _run(
        ["gh", "pr", "view", str(pr_number), "--json", "headRefName", "--jq", ".headRefName"],
        cwd=repo_path,
    )
    if result.returncode != 0:
        raise GitError(f"Failed to get PR #{pr_number}: {result.stderr.strip()}")
    branch = result.stdout.strip()
    if not branch:
        raise GitError(f"PR #{pr_number} has no head branch")
    return branch


def fetch_and_pull(repo_path: str, branch: str) -> None:
    """Fetch origin and pull the given branch."""
    _run(["git", "fetch", "origin"], cwd=repo_path)
    _run(["git", "pull", "origin", branch], cwd=repo_path)


def fetch_branch(repo_path: str, branch: str) -> None:
    """Fetch a branch from origin so it's available locally."""
    _run(["git", "fetch", "origin", branch], cwd=repo_path)


def is_branch_merged(repo_path: str, branch: str, into: str) -> bool:
    """Return True if branch has been merged into the given target branch."""
    result = _run(
        ["git", "branch", "--merged", into, "--format=%(refname:short)"],
        cwd=repo_path,
    )
    if result.returncode != 0:
        raise GitError(
            f"Failed to check merged branches in '{repo_path}': {result.stderr.strip()}"
        )
    merged = result.stdout.strip().splitlines()
    return branch in merged


def has_remote_branch(repo_path: str, branch: str) -> bool:
    """Return True if the given branch exists on the remote (origin)."""
    result = _run(
        ["git", "ls-remote", "--heads", "origin", branch],
        cwd=repo_path,
    )
    if result.returncode != 0:
        raise GitError(
            f"Failed to query remote for branch '{branch}' in '{repo_path}': "
            f"{result.stderr.strip()}"
        )
    return bool(result.stdout.strip())
