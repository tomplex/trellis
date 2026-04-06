"""Thin wrapper around tmux CLI commands."""

from __future__ import annotations

import subprocess


class TmuxError(Exception):
    """Raised when a tmux command fails."""


def _run(args: list[str], check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        args,
        capture_output=True,
        text=True,
        check=False,
    )


def list_sessions() -> list[dict]:
    """Return a list of tmux sessions with name, window count, and attached status."""
    result = _run(
        ["tmux", "list-sessions", "-F", "#{session_name}\t#{session_windows}\t#{session_attached}"]
    )
    if result.returncode != 0:
        # No server running or no sessions — return empty list
        return []
    sessions = []
    for line in result.stdout.strip().splitlines():
        if not line:
            continue
        name, windows, attached = line.split("\t")
        sessions.append(
            {
                "name": name,
                "windows": int(windows),
                "attached": attached == "1",
            }
        )
    return sessions


def session_exists(name: str) -> bool:
    """Return True if a tmux session with the given name exists."""
    result = _run(["tmux", "has-session", "-t", name])
    return result.returncode == 0


def new_session(name: str, start_dir: str) -> None:
    """Create a new tmux session. Raises TmuxError if the session already exists."""
    if session_exists(name):
        raise TmuxError(f"Session '{name}' already exists")
    result = _run(["tmux", "new-session", "-d", "-s", name, "-c", start_dir])
    if result.returncode != 0:
        raise TmuxError(f"Failed to create session '{name}': {result.stderr.strip()}")


def switch_client(session_name: str) -> None:
    """Switch the tmux client to the given session."""
    result = _run(["tmux", "switch-client", "-t", session_name])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to switch to session '{session_name}': {result.stderr.strip()}"
        )


def new_window(session_name: str, window_name: str, start_dir: str) -> None:
    """Create a new window in the given session."""
    result = _run(
        [
            "tmux",
            "new-window",
            "-t",
            session_name,
            "-n",
            window_name,
            "-c",
            start_dir,
        ]
    )
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to create window '{window_name}' in session '{session_name}': "
            f"{result.stderr.strip()}"
        )


def kill_session(session_name: str) -> None:
    """Kill the named tmux session."""
    result = _run(["tmux", "kill-session", "-t", session_name])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to kill session '{session_name}': {result.stderr.strip()}"
        )
