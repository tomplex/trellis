"""Conversation history browser."""

from __future__ import annotations

import re
import subprocess

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Input, Static

from torchard.core.db import get_repos, get_sessions
from torchard.core.history import Conversation, filter_by_paths, parse_index
from torchard.core.manager import Manager
from torchard.tui.switch import write_switch


def _safe_id(text: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_-]", "_", text)


def _truncate(text: str, max_len: int) -> str:
    if len(text) <= max_len:
        return text
    return text[: max_len - 1] + "…"


class HistoryScreen(Screen):
    """Browse and resume past Claude conversations."""

    BINDINGS = [
        Binding("escape", "dismiss", "Back"),
        Binding("enter", "resume", "Resume"),
        Binding("j,down", "cursor_down", "Down", show=False),
        Binding("k,up", "cursor_up", "Up", show=False),
        Binding("t", "toggle_scope", "Toggle scope"),
        Binding("d", "sort_date", "Sort date"),
        Binding("p", "sort_project", "Sort project"),
        Binding("b", "sort_branch", "Sort branch"),
        Binding("s", "sort_summary", "Sort summary"),
    ]

    SORT_KEYS = {
        "date": lambda e: e.date,
        "project": lambda e: e.project.lower(),
        "branch": lambda e: e.branch.lower(),
        "summary": lambda e: e.summary.lower(),
    }

    def __init__(
        self,
        manager: Manager,
        scope_paths: list[str] | None = None,
        scope_label: str | None = None,
    ) -> None:
        super().__init__()
        self._manager = manager
        self._scope_paths = scope_paths
        self._scope_label = scope_label
        self._scoped = scope_paths is not None
        self._all_entries: list[Conversation] = []
        self._scoped_entries: list[Conversation] = []
        self._displayed: list[Conversation] = []
        self._filter = ""
        self._sort_by = "date"
        self._sort_reverse = True  # newest first by default

    def compose(self) -> ComposeResult:
        yield Static("", id="history-title")
        yield Input(placeholder="Filter…", id="history-filter", classes="hidden")
        yield DataTable(id="history-table", cursor_type="row", zebra_stripes=False)
        yield Footer()

    def on_mount(self) -> None:
        self._all_entries = parse_index()
        if self._scope_paths:
            self._scoped_entries = filter_by_paths(self._all_entries, self._scope_paths)
        else:
            self._scoped_entries = self._all_entries

        table = self.query_one(DataTable)
        table.add_columns("Date", "Project", "Branch", "Summary")
        self._rebuild()
        table.focus()

    def _rebuild(self) -> None:
        entries = self._scoped_entries if self._scoped else self._all_entries

        if self._filter:
            q = self._filter
            entries = [
                e for e in entries
                if q in e.date.lower()
                or q in e.project.lower()
                or q in e.branch.lower()
                or q in e.summary.lower()
            ]

        sort_fn = self.SORT_KEYS[self._sort_by]
        entries = sorted(entries, key=sort_fn, reverse=self._sort_reverse)
        self._displayed = entries
        self._update_title()

        table = self.query_one(DataTable)
        table.clear()

        for i, entry in enumerate(entries):
            # Shorten project path
            proj = entry.project.replace("/Users/tom/", "~/")
            table.add_row(
                entry.date,
                _truncate(proj, 40),
                _truncate(entry.branch, 20),
                _truncate(entry.summary, 50),
                key=str(i),
            )

        if entries:
            table.move_cursor(row=0)

    def _update_title(self) -> None:
        count = len(self._displayed)
        if self._scoped and self._scope_label:
            scope_text = f"[dim]scoped to[/dim] {self._scope_label}"
        else:
            scope_text = "[dim]all conversations[/dim]"
        arrow = "↓" if self._sort_reverse else "↑"
        title = f"[bold]History[/bold]  {scope_text}  [dim]({count})[/dim]  [dim]sort: {self._sort_by} {arrow}[/dim]"
        self.query_one("#history-title", Static).update(title)

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "history-filter":
            self._filter = event.value.lower()
            self._rebuild()

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()

    def _sort(self, key: str) -> None:
        if self._sort_by == key:
            self._sort_reverse = not self._sort_reverse
        else:
            self._sort_by = key
            self._sort_reverse = key == "date"  # date defaults descending, others ascending
        self._rebuild()

    def action_sort_date(self) -> None:
        self._sort("date")

    def action_sort_project(self) -> None:
        self._sort("project")

    def action_sort_branch(self) -> None:
        self._sort("branch")

    def action_sort_summary(self) -> None:
        self._sort("summary")

    def action_toggle_scope(self) -> None:
        if not self._scope_paths:
            return  # no scope available, toggle does nothing
        self._scoped = not self._scoped
        self._rebuild()

    def action_resume(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        idx = int(row_key)
        entry = self._displayed[idx]

        # Find if this project dir belongs to an existing managed session
        sessions = get_sessions(self._manager._conn)
        repos = {r.id: r for r in get_repos(self._manager._conn)}
        target_session = None

        for s in sessions:
            repo = repos.get(s.repo_id)
            if repo and entry.project.startswith(repo.path):
                target_session = s.name
                break
            # Check worktree paths too
            from torchard.core.db import get_worktrees_for_session
            for wt in get_worktrees_for_session(self._manager._conn, s.id):
                if entry.project.startswith(wt.path):
                    target_session = s.name
                    break
            if target_session:
                break

        session_id = entry.session_id
        resume_cmd = f"claude --resume {session_id}"

        if target_session:
            # Open in existing session
            from torchard.core import tmux
            subprocess.run([
                "tmux", "new-window", "-t", target_session,
                "-n", f"resume-{session_id[:8]}",
                "-c", entry.project,
            ])
            subprocess.run([
                "tmux", "send-keys", "-t",
                f"{target_session}:resume-{session_id[:8]}",
                resume_cmd, "Enter",
            ])
            write_switch({"type": "session", "target": target_session})
        else:
            # Create a new tmux session
            session_name = f"resume-{session_id[:8]}"
            subprocess.run([
                "tmux", "new-session", "-d",
                "-s", session_name,
                "-c", entry.project,
            ])
            subprocess.run([
                "tmux", "send-keys", "-t", session_name,
                resume_cmd, "Enter",
            ])
            write_switch({"type": "session", "target": session_name})

        self.app.exit()

    def action_dismiss(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    HistoryScreen {
        background: #1a1a2e;
    }
    #history-title {
        color: #00aaff;
        text-style: bold;
        padding: 1 2 0 2;
    }
    #history-filter {
        dock: top;
        margin: 0 1;
        height: 3;
    }
    DataTable {
        background: #1a1a2e;
        color: #e0e0e0;
        height: 1fr;
    }
    DataTable > .datatable--header {
        background: #16213e;
        color: #00aaff;
        text-style: bold;
    }
    DataTable > .datatable--cursor {
        background: #0f3460;
        color: #ffffff;
    }
    DataTable > .datatable--hover {
        background: #16213e;
    }
    Footer {
        background: #16213e;
        color: #aaaaaa;
    }
    Footer > .footer--highlight {
        background: #0f3460;
        color: #00aaff;
    }
    Footer > .footer--key {
        color: #00aaff;
        text-style: bold;
    }
    """
