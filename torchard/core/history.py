"""Parse Claude's conversation-index.md into structured entries."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

_HEADER_RE = re.compile(r"^## (\d{4}-\d{2}-\d{2} \d{2}:\d{2}) \[([0-9a-f]+)\]")
_PROJECT_RE = re.compile(r"^- \*\*project\*\*: `(.+)`")
_BRANCH_RE = re.compile(r"^- \*\*branch\*\*: `(.+)`")
_INTENT_LINE_RE = re.compile(r"^  - (.+)")
_FILES_RE = re.compile(r"^- \*\*files\*\*: (.+)")

INDEX_PATH = Path.home() / ".claude" / "conversation-index.md"


@dataclass
class Conversation:
    date: str  # "2026-02-09 16:05"
    session_id: str  # short hex like "e8ee4cb2"
    project: str  # directory path
    branch: str
    intents: list[str] = field(default_factory=list)
    files: list[str] = field(default_factory=list)

    @property
    def summary(self) -> str:
        """First non-interrupted intent, truncated."""
        for intent in self.intents:
            if not intent.startswith("[Request interrupted"):
                return intent
        return self.intents[0] if self.intents else ""


def parse_index(path: Path | None = None) -> list[Conversation]:
    """Parse conversation-index.md into a list of Conversation entries, newest first."""
    if path is None:
        path = INDEX_PATH
    if not path.exists():
        return []

    entries: list[Conversation] = []
    current: Conversation | None = None
    in_intent = False

    for line in path.read_text().splitlines():
        header = _HEADER_RE.match(line)
        if header:
            if current is not None:
                entries.append(current)
            current = Conversation(
                date=header.group(1),
                session_id=header.group(2),
                project="",
                branch="",
            )
            in_intent = False
            continue

        if current is None:
            continue

        project = _PROJECT_RE.match(line)
        if project:
            current.project = project.group(1)
            in_intent = False
            continue

        branch = _BRANCH_RE.match(line)
        if branch:
            current.branch = branch.group(1)
            in_intent = False
            continue

        if line.strip() == "- **intent**:":
            in_intent = True
            continue

        if in_intent:
            intent = _INTENT_LINE_RE.match(line)
            if intent:
                current.intents.append(intent.group(1))
                continue
            else:
                in_intent = False

        files = _FILES_RE.match(line)
        if files:
            current.files = [f.strip() for f in files.group(1).split(",")]
            in_intent = False

    if current is not None:
        entries.append(current)

    # Newest first
    entries.reverse()
    return entries


def filter_by_paths(entries: list[Conversation], paths: list[str]) -> list[Conversation]:
    """Filter conversations whose project starts with any of the given paths."""
    return [e for e in entries if any(e.project.startswith(p) for p in paths)]
