"""Utilities for reading Claude session data."""

from __future__ import annotations

import json
import re
from pathlib import Path


def get_session_id(pane_pid: str) -> str | None:
    """Look up the Claude session UUID for a given pane PID."""
    if not pane_pid:
        return None
    pid_file = Path("/tmp/claude-sessions") / f"pid-{pane_pid}"
    try:
        return pid_file.read_text().strip() if pid_file.exists() else None
    except OSError:
        return None


def get_first_user_message(session_id: str) -> str | None:
    """Read the first user message from a Claude session's JSONL file."""
    projects_dir = Path.home() / ".claude" / "projects"
    if not projects_dir.exists():
        return None
    for project_dir in projects_dir.iterdir():
        if not project_dir.is_dir():
            continue
        jsonl = project_dir / f"{session_id}.jsonl"
        if jsonl.exists():
            return _first_user_message(jsonl)
    return None


def summarize_message(message: str, max_words: int = 4) -> str:
    """Turn a user message into a short kebab-case name."""
    first_line = message.split("\n")[0].strip().lstrip("#").strip()
    words = first_line.split()[:max_words]
    name = "-".join(w.lower().strip(".,!?:;\"'()[]{}") for w in words if w)
    if len(name) > 30:
        name = name[:30].rsplit("-", 1)[0]
    return name or "claude"


def classify_pane(pane_text: str) -> str:
    """Classify a claude pane's state from its captured terminal content.

    Returns one of: "idle", "working", "prompting".

    The ``❯`` prompt is always visible, so we can't use it to detect state.
    Instead we look at the spinner line just above the status bar:
      - Working: spinner with ``…`` (e.g. "✻ Envisioning…")
      - Idle: spinner shows past tense with duration (e.g. "✻ Brewed for 31s")
      - Prompting: permission dialog with numbered choices and "Esc to cancel"
    """
    lines = [l for l in pane_text.splitlines() if l.strip()]
    tail = lines[-10:] if len(lines) > 10 else lines
    if not tail:
        return "idle"

    tail_text = "\n".join(tail)

    # Permission dialog: numbered selection + "Esc to cancel"
    if re.search(r"❯\s+1\.", tail_text) and "Esc to cancel" in tail_text:
        return "prompting"

    # Active spinner: a non-ASCII symbol followed by a word ending in … (ellipsis)
    if re.search(r"[^\x00-\x7f]\s+\S+…", tail_text):
        return "working"

    return "idle"


def _first_user_message(jsonl_path: Path) -> str | None:
    with open(jsonl_path) as f:
        for line in f:
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            if entry.get("type") == "user":
                content = entry.get("message", {}).get("content", "")
                if isinstance(content, str) and content.strip():
                    return content.strip()
    return None
