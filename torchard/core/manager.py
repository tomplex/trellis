"""High-level orchestration: ties DB, tmux, and git together."""

from __future__ import annotations

import os
import sqlite3
from datetime import datetime, timezone
from pathlib import Path

from torchard.core import git, tmux
from torchard.core.db import (
    add_repo,
    add_session,
    add_worktree,
    delete_session as db_delete_session,
    delete_worktree as db_delete_worktree,
    get_config,
    get_repos,
    get_sessions,
    get_worktrees,
    get_worktrees_for_session,
)
from torchard.core.models import Repo, Session, Worktree


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def detect_subsystems(repo_path: str) -> list[str]:
    """Detect subsystem directories in a monorepo (e.g. workers/model_train, src/sojourner)."""
    root = Path(repo_path)
    subsystems = []
    for parent_name in ("workers", "src", "libs", "pods"):
        parent = root / parent_name
        if parent.is_dir():
            for child in sorted(parent.iterdir()):
                if child.is_dir() and not child.name.startswith("."):
                    subsystems.append(f"{parent_name}/{child.name}")
    return subsystems


class Manager:
    def __init__(self, conn: sqlite3.Connection) -> None:
        self._conn = conn

    @property
    def repos_dir(self) -> Path:
        return Path(get_config(self._conn, "repos_dir") or str(Path.home() / "dev"))

    @property
    def worktrees_dir(self) -> Path:
        return Path(get_config(self._conn, "worktrees_dir") or str(Path.home() / "dev" / "worktrees"))

    def _worktree_path(self, repo_name: str, branch: str) -> str:
        return str(self.worktrees_dir / repo_name / branch)

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _get_repo_by_id(self, repo_id: int) -> Repo | None:
        for r in get_repos(self._conn):
            if r.id == repo_id:
                return r
        return None

    def _get_repo_by_path(self, path: str) -> Repo | None:
        for r in get_repos(self._conn):
            if r.path == path:
                return r
        return None

    def _get_session_by_id(self, session_id: int) -> Session | None:
        for s in get_sessions(self._conn):
            if s.id == session_id:
                return s
        return None

    def _get_worktree_by_id(self, worktree_id: int) -> Worktree | None:
        for wt in get_worktrees(self._conn):
            if wt.id == worktree_id:
                return wt
        return None

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def create_session(self, repo_path: str, base_branch: str, session_name: str, subdirectory: str | None = None) -> Session:
        """Register repo if needed, create worktree, create DB session, create tmux session."""
        repo = self._get_repo_by_path(repo_path)
        if repo is None:
            default_branch = git.detect_default_branch(repo_path)
            name = Path(repo_path).name
            repo = add_repo(self._conn, Repo(path=repo_path, name=name, default_branch=default_branch))

        # If the base branch is the repo's default branch, start in the repo root.
        # Otherwise create a worktree for the feature branch.
        default = repo.default_branch
        created_worktree = False
        if base_branch == default:
            start_dir = repo_path
        else:
            # Fetch latest before creating worktree
            git.fetch_and_pull(repo_path, default)

            worktree_path = self._worktree_path(repo.name, base_branch)
            try:
                git.create_worktree(repo_path, worktree_path, base_branch, default)
                created_worktree = True
            except git.GitError:
                # Branch/worktree may already exist - use it if the dir is there
                if not Path(worktree_path).exists():
                    raise
            start_dir = worktree_path

        session = add_session(
            self._conn,
            Session(
                name=session_name,
                repo_id=repo.id,
                base_branch=base_branch,
                created_at=_now(),
            ),
        )

        effective_dir = start_dir
        if subdirectory:
            effective_dir = str(Path(start_dir) / subdirectory)

        if start_dir != repo_path:
            # Feature branch: 3-window layout (claude, diff, shell)
            tmux.new_session(session_name, effective_dir)
            tmux.rename_window(session_name, 1, "claude")
            tmux.send_keys(f"{session_name}:claude", "claude", "Enter")
            tmux.new_window(session_name, "shell", effective_dir)
            tmux.select_window(session_name, 1)
        else:
            tmux.new_session(session_name, effective_dir)

        # Record worktree if we created one
        if start_dir != repo_path:
            add_worktree(
                self._conn,
                Worktree(
                    repo_id=repo.id,
                    path=start_dir,
                    branch=base_branch,
                    session_id=session.id,
                    created_at=_now(),
                ),
            )

        return session

    def adopt_session(self, session_name: str, repo_path: str, base_branch: str) -> Session:
        """Adopt an existing tmux session into torchard's management."""
        repo = self._get_repo_by_path(repo_path)
        if repo is None:
            default_branch = git.detect_default_branch(repo_path)
            name = Path(repo_path).name
            repo = add_repo(self._conn, Repo(path=repo_path, name=name, default_branch=default_branch))

        session = add_session(
            self._conn,
            Session(
                name=session_name,
                repo_id=repo.id,
                base_branch=base_branch,
                created_at=_now(),
            ),
        )
        return session

    def rename_session(self, session_id: int, new_name: str) -> Session:
        """Rename a session in both tmux and the DB."""
        session = self._get_session_by_id(session_id)
        if session is None:
            raise ValueError(f"Session {session_id} not found")

        # Rename in tmux (if live)
        try:
            tmux.rename_session(session.name, new_name)
        except tmux.TmuxError:
            pass  # session may not be live

        # Rename in DB
        self._conn.execute(
            "UPDATE sessions SET name = ? WHERE id = ?",
            (new_name, session_id),
        )
        self._conn.commit()
        session.name = new_name
        return session

    def set_base_branch(self, session_id: int, base_branch: str) -> Session:
        """Update a session's base branch."""
        session = self._get_session_by_id(session_id)
        if session is None:
            raise ValueError(f"Session {session_id} not found")
        self._conn.execute(
            "UPDATE sessions SET base_branch = ? WHERE id = ?",
            (base_branch, session_id),
        )
        self._conn.commit()
        session.base_branch = base_branch
        return session

    def checkout_and_review(self, repo_path: str, pr_or_branch: str) -> tuple[Session, str]:
        """Create a worktree + session for a PR number or branch, ready for review.

        Returns (session, worktree_path) so the caller can launch claude in it.
        """
        repo = self._get_repo_by_path(repo_path)
        if repo is None:
            default_branch = git.detect_default_branch(repo_path)
            name = Path(repo_path).name
            repo = add_repo(self._conn, Repo(path=repo_path, name=name, default_branch=default_branch))

        # Resolve PR number to branch name
        if pr_or_branch.isdigit():
            branch = git.get_pr_branch(repo_path, int(pr_or_branch))
        else:
            branch = pr_or_branch

        # Fetch the branch so it's available locally
        git.fetch_branch(repo_path, branch)

        # Create worktree
        worktree_path = self._worktree_path(repo.name, branch)
        base = f"origin/{branch}"
        try:
            git.create_worktree(repo_path, worktree_path, branch, base)
        except git.GitError:
            # Worktree or branch may already exist - try to just use the path
            if not Path(worktree_path).exists():
                raise

        # Sanitize session name
        session_name = tmux.sanitize_session_name(branch)

        # Create session
        session = add_session(
            self._conn,
            Session(
                name=session_name,
                repo_id=repo.id,
                base_branch=branch,
                created_at=_now(),
            ),
        )
        tmux.new_session(session_name, worktree_path)
        tmux.rename_window(session_name, 1, "claude")
        tmux.new_window(session_name, "shell", worktree_path)
        tmux.select_window(session_name, 1)

        # Record worktree
        add_worktree(
            self._conn,
            Worktree(
                repo_id=repo.id,
                path=worktree_path,
                branch=branch,
                session_id=session.id,
                created_at=_now(),
            ),
        )

        return session, worktree_path

    def add_tab(self, session_id: int, branch_name: str) -> Worktree:
        """Create a worktree + tmux window for the given session."""
        session = self._get_session_by_id(session_id)
        if session is None:
            raise ValueError(f"Session {session_id} not found")

        repo = self._get_repo_by_id(session.repo_id)
        if repo is None:
            raise ValueError(f"Repo {session.repo_id} not found")

        worktree_path = Path(self._worktree_path(repo.name, branch_name))
        git.create_worktree(repo.path, str(worktree_path), branch_name, session.base_branch)
        tmux.new_window(session.name, branch_name, str(worktree_path))

        worktree = add_worktree(
            self._conn,
            Worktree(
                repo_id=repo.id,
                path=str(worktree_path),
                branch=branch_name,
                session_id=session_id,
                created_at=_now(),
            ),
        )
        return worktree

    def delete_session(self, session_id: int, cleanup_worktrees: bool = False) -> None:
        """Kill tmux session, optionally remove worktrees, remove DB session."""
        session = self._get_session_by_id(session_id)
        if session is None:
            raise ValueError(f"Session {session_id} not found")

        repo = self._get_repo_by_id(session.repo_id)

        if cleanup_worktrees:
            worktrees = get_worktrees_for_session(self._conn, session_id)
            for wt in worktrees:
                if repo is not None:
                    try:
                        git.remove_worktree(repo.path, wt.path)
                    except git.GitError:
                        pass
                db_delete_worktree(self._conn, wt.id)
        else:
            # Detach worktrees from this session to satisfy FK constraint
            self._conn.execute(
                "UPDATE worktrees SET session_id = NULL WHERE session_id = ?",
                (session_id,),
            )
            self._conn.commit()

        try:
            tmux.kill_session(session.name)
        except Exception:
            pass

        db_delete_session(self._conn, session_id)

    def cleanup_worktree(self, worktree_id: int) -> None:
        """Remove git worktree and delete DB record."""
        wt = self._get_worktree_by_id(worktree_id)
        if wt is None:
            raise ValueError(f"Worktree {worktree_id} not found")

        repo = self._get_repo_by_id(wt.repo_id)
        if repo is None:
            raise ValueError(f"Repo {wt.repo_id} not found")

        git.remove_worktree(repo.path, wt.path)
        db_delete_worktree(self._conn, worktree_id)

    def get_stale_worktrees(self) -> list[Worktree]:
        """Return worktrees whose branch is merged or whose remote branch is deleted."""
        all_worktrees = get_worktrees(self._conn)
        stale = []
        for wt in all_worktrees:
            repo = self._get_repo_by_id(wt.repo_id)
            if repo is None:
                continue
            try:
                merged = git.is_branch_merged(repo.path, wt.branch, repo.default_branch)
            except git.GitError:
                merged = False
            try:
                has_remote = git.has_remote_branch(repo.path, wt.branch)
            except git.GitError:
                has_remote = True  # assume not stale on error

            if merged or not has_remote:
                stale.append(wt)
        return stale

    def scan_existing(self) -> None:
        """First-run adoption: scan repos and worktrees dirs, scan live tmux sessions,
        and populate the DB with discovered state."""
        home_dev = self.repos_dir
        worktrees_root = self.worktrees_dir

        # Build a path -> Repo map from already-known repos
        known_repos: dict[str, Repo] = {r.path: r for r in get_repos(self._conn)}
        known_worktree_paths = {wt.path for wt in get_worktrees(self._conn)}

        # 1. Scan ~/dev/ for git repos (direct children that are git repos)
        if home_dev.is_dir():
            for entry in home_dev.iterdir():
                if not entry.is_dir() or entry.name == "worktrees":
                    continue
                if (entry / ".git").exists() and str(entry) not in known_repos:
                    try:
                        default_branch = git.detect_default_branch(str(entry))
                    except git.GitError:
                        default_branch = "main"
                    repo = add_repo(
                        self._conn,
                        Repo(path=str(entry), name=entry.name, default_branch=default_branch),
                    )
                    known_repos[str(entry)] = repo

        # 2. Scan ~/dev/worktrees/<repo>/<branch>/ and adopt worktrees
        if worktrees_root.is_dir():
            for repo_dir in worktrees_root.iterdir():
                if not repo_dir.is_dir():
                    continue
                for branch_dir in repo_dir.iterdir():
                    if not branch_dir.is_dir():
                        continue
                    if str(branch_dir) in known_worktree_paths:
                        continue
                    # Find or create the repo record (look in ~/dev/<repo_name>)
                    repo_path_candidate = str(home_dev / repo_dir.name)
                    if repo_path_candidate not in known_repos:
                        # Try to detect from actual path
                        if Path(repo_path_candidate).is_dir():
                            try:
                                default_branch = git.detect_default_branch(repo_path_candidate)
                            except git.GitError:
                                default_branch = "main"
                            repo = add_repo(
                                self._conn,
                                Repo(
                                    path=repo_path_candidate,
                                    name=repo_dir.name,
                                    default_branch=default_branch,
                                ),
                            )
                            known_repos[repo_path_candidate] = repo
                        else:
                            continue  # Can't resolve repo, skip
                    repo = known_repos[repo_path_candidate]
                    add_worktree(
                        self._conn,
                        Worktree(
                            repo_id=repo.id,
                            path=str(branch_dir),
                            branch=branch_dir.name,
                            created_at=_now(),
                        ),
                    )
                    known_worktree_paths.add(str(branch_dir))

        # 3. Scan live tmux sessions; match to repos by cwd
        known_session_names = {s.name for s in get_sessions(self._conn)}
        live_sessions = tmux.list_sessions()
        for ts in live_sessions:
            if ts["name"] in known_session_names:
                continue
            # Try to figure out which repo this session belongs to by matching
            # the session name to a known repo name (best-effort heuristic)
            matched_repo: Repo | None = None
            for repo in known_repos.values():
                if ts["name"].startswith(repo.name):
                    matched_repo = repo
                    break

            if matched_repo is None:
                continue

            add_session(
                self._conn,
                Session(
                    name=ts["name"],
                    repo_id=matched_repo.id,
                    base_branch=matched_repo.default_branch,
                    created_at=_now(),
                ),
            )
            known_session_names.add(ts["name"])

    def list_sessions(self) -> list[dict]:
        """Return DB sessions enriched with live tmux state, plus unmanaged live sessions."""
        db_sessions = get_sessions(self._conn)
        live_by_name: dict[str, dict] = {s["name"]: s for s in tmux.list_sessions()}
        db_names: set[str] = set()

        result = []
        for session in db_sessions:
            db_names.add(session.name)
            entry: dict = {
                "id": session.id,
                "name": session.name,
                "repo_id": session.repo_id,
                "base_branch": session.base_branch,
                "created_at": session.created_at,
                "last_selected_at": session.last_selected_at,
                "windows": None,
                "attached": False,
                "live": False,
                "managed": True,
            }
            live = live_by_name.get(session.name)
            if live:
                entry["windows"] = live["windows"]
                entry["attached"] = live["attached"]
                entry["live"] = True
            result.append(entry)

        # Include live tmux sessions not tracked in the DB
        for name, live in live_by_name.items():
            if name not in db_names:
                result.append({
                    "id": None,
                    "name": name,
                    "repo_id": None,
                    "base_branch": None,
                    "created_at": None,
                    "last_selected_at": None,
                    "windows": live["windows"],
                    "attached": live["attached"],
                    "live": True,
                    "managed": False,
                })

        return result
