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

    Returns one of: "thinking", "working", "waiting", "prompting", "idle"
    """
    lines = [l.strip() for l in pane_text.strip().splitlines() if l.strip()]
    if not lines:
        return "idle"

    tail = "\n".join(lines[-6:])

    # Actively generating output
    if "Generating" in tail or "Streaming" in tail:
        return "thinking"

    # Running a tool
    if re.search(r"Running", tail) and re.search(r"⏺|⎿", tail):
        return "working"

    # Waiting for permission / confirmation
    if "Enter to confirm" in tail or "to approve" in tail or re.search(r"[Yy]es.*[Nn]o", tail):
        return "prompting"

    # At the input prompt (❯ with status bar)
    if re.search(r"^❯\s*$", tail, re.MULTILINE):
        return "waiting"

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
