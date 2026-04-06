from dataclasses import dataclass


@dataclass
class Repo:
    path: str
    name: str
    default_branch: str
    id: int | None = None


@dataclass
class Session:
    name: str
    repo_id: int
    base_branch: str
    created_at: str
    id: int | None = None


@dataclass
class Worktree:
    repo_id: int
    path: str
    branch: str
    created_at: str
    session_id: int | None = None
    tmux_window: int | None = None
    id: int | None = None
