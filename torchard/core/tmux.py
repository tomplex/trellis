"""Thin wrapper around tmux CLI commands."""

from __future__ import annotations

import re
import subprocess


def sanitize_session_name(name: str) -> str:
    """Remove/replace characters not allowed in tmux session names."""
    name = re.sub(r"[.:]", "-", name)
    name = name.strip(" -")
    return name or "new-session"


class TmuxError(Exception):
    """Raised when a tmux command fails."""


def _run(args: list[str]) -> subprocess.CompletedProcess:
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


def new_window(session_name: str, window_name: str, start_dir: str | None = None) -> None:
    """Create a new window in the given session."""
    cmd = ["tmux", "new-window", "-t", session_name, "-n", window_name]
    if start_dir:
        cmd += ["-c", start_dir]
    result = _run(cmd)
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to create window '{window_name}' in session '{session_name}': "
            f"{result.stderr.strip()}"
        )


def select_window(session_name: str, window_index: int) -> None:
    """Select a specific window in a session."""
    result = _run(["tmux", "select-window", "-t", f"{session_name}:{window_index}"])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to select window {window_index} in '{session_name}': {result.stderr.strip()}"
        )


def rename_window(session_name: str, window_index: int, new_name: str) -> None:
    """Rename a tmux window."""
    result = _run(["tmux", "rename-window", "-t", f"{session_name}:{window_index}", new_name])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to rename window {window_index} in '{session_name}': {result.stderr.strip()}"
        )


def rename_session(old_name: str, new_name: str) -> None:
    """Rename a tmux session."""
    result = _run(["tmux", "rename-session", "-t", old_name, new_name])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to rename session '{old_name}' to '{new_name}': {result.stderr.strip()}"
        )


def list_all_windows() -> dict[str, list[dict]]:
    """Return all windows across all sessions, keyed by session name. Single tmux call."""
    result = _run([
        "tmux", "list-windows", "-a",
        "-F", "#{session_name}\t#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}",
    ])
    if result.returncode != 0:
        return {}
    by_session: dict[str, list[dict]] = {}
    for line in result.stdout.strip().splitlines():
        if not line:
            continue
        parts = line.split("\t")
        session_name = parts[0]
        index, name, path = parts[1], parts[2], parts[3]
        command = parts[4] if len(parts) > 4 else ""
        pane_pid = parts[5] if len(parts) > 5 else ""
        by_session.setdefault(session_name, []).append(
            {"index": int(index), "name": name, "path": path, "command": command, "pane_pid": pane_pid}
        )
    return by_session


def list_windows(session_name: str) -> list[dict]:
    """Return windows in a session with index, name, current path, running command, and pane PID."""
    result = _run([
        "tmux", "list-windows", "-t", session_name,
        "-F", "#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}",
    ])
    if result.returncode != 0:
        return []
    windows = []
    for line in result.stdout.strip().splitlines():
        if not line:
            continue
        parts = line.split("\t")
        index, name, path = parts[0], parts[1], parts[2]
        command = parts[3] if len(parts) > 3 else ""
        pane_pid = parts[4] if len(parts) > 4 else ""
        windows.append({"index": int(index), "name": name, "path": path, "command": command, "pane_pid": pane_pid})
    return windows


def kill_window(session_name: str, window_index: int) -> None:
    """Kill a specific window in a session."""
    result = _run(["tmux", "kill-window", "-t", f"{session_name}:{window_index}"])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to kill window {window_index} in '{session_name}': {result.stderr.strip()}"
        )


def send_keys(target: str, *keys: str) -> None:
    """Send keys to a tmux target (e.g. 'session:window')."""
    _run(["tmux", "send-keys", "-t", target, *keys])



def capture_pane(target: str, lines: int = 5) -> str:
    """Capture the last N lines of a pane's visible content."""
    result = _run(["tmux", "capture-pane", "-t", target, "-p", "-J", f"-S", f"-{lines}"])
    return result.stdout if result.returncode == 0 else ""


def get_pane_pid(target: str) -> str | None:
    """Get the PID of the active pane in a target."""
    result = _run(["tmux", "display-message", "-t", target, "-p", "#{pane_pid}"])
    pid = result.stdout.strip() if result.returncode == 0 else ""
    return pid or None


def kill_session(session_name: str) -> None:
    """Kill the named tmux session."""
    result = _run(["tmux", "kill-session", "-t", session_name])
    if result.returncode != 0:
        raise TmuxError(
            f"Failed to kill session '{session_name}': {result.stderr.strip()}"
        )
