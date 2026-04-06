"""Main session list screen."""

from __future__ import annotations

import json
import re
import tempfile
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static
from textual.containers import Vertical

from torchard.core import tmux
from torchard.core.db import get_repos, get_worktrees_for_session
from torchard.core.manager import Manager
from torchard.tui.views.adopt_session import AdoptSessionScreen
from torchard.tui.views.cleanup import CleanupScreen
from torchard.tui.views.confirm import ConfirmModal
from torchard.tui.views.edit_branch import EditBranchScreen
from torchard.tui.views.new_session import NewSessionScreen
from torchard.tui.views.new_tab import NewTabScreen
from torchard.tui.views.rename_session import RenameSessionScreen, RenameWindowScreen


_HELP_TEXT = """\
[bold #00aaff]torchard[/bold #00aaff] — tmux session & worktree manager

[bold]Session List[/bold]
  [#00aaff]enter[/#00aaff]     Switch to session
  [#00aaff]tab[/#00aaff]       Expand/collapse worktrees
  [#00aaff]n[/#00aaff]         New managed session
  [#00aaff]w[/#00aaff]         New worktree tab in session
  [#00aaff]d[/#00aaff]         Delete session
  [#00aaff]r[/#00aaff]         Rename session
  [#00aaff]b[/#00aaff]         Change base branch
  [#00aaff]a[/#00aaff]         Adopt unmanaged session
  [#00aaff]c[/#00aaff]         Cleanup stale worktrees
  [#00aaff]j/k[/#00aaff]       Navigate up/down
  [#00aaff]q[/#00aaff]         Quit

[bold]Cleanup View[/bold]
  [#00aaff]space/enter[/#00aaff]  Toggle selection
  [#00aaff]a[/#00aaff]           Select all
  [#00aaff]d[/#00aaff]           Delete selected
  [#00aaff]escape[/#00aaff]      Back

[dim]Press Escape to close this help.[/dim]\
"""


class HelpScreen(Screen):
    """Keybind reference."""

    BINDINGS = [
        Binding("escape", "dismiss", "Close"),
        Binding("q", "dismiss", "Close", show=False),
    ]

    def compose(self) -> ComposeResult:
        with Vertical(id="help-container"):
            yield Static(_HELP_TEXT, id="help-text")
        yield Footer()

    def action_dismiss(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    HelpScreen {
        align: center middle;
    }
    #help-container {
        width: 50;
        height: auto;
        border: solid #00aaff;
        padding: 1 2;
        background: #16213e;
    }
    #help-text {
        color: #e0e0e0;
    }
    """


class SessionListScreen(Screen):
    """The main session list view."""

    BINDINGS = [
        Binding("q", "quit", "Quit"),
        Binding("escape", "quit", "Quit", show=False),
        Binding("j,down", "cursor_down", "Down", show=False),
        Binding("k,up", "cursor_up", "Up", show=False),
        Binding("enter", "select", "Switch"),
        Binding("tab", "toggle_expand", "Expand"),
        Binding("n", "new_session", "New"),
        Binding("w", "new_tab", "Tab"),
        Binding("d", "delete_session", "Delete"),
        Binding("r", "rename", "Rename"),
        Binding("b", "edit_branch", "Branch"),
        Binding("a", "adopt", "Adopt"),
        Binding("c", "cleanup", "Cleanup"),
        Binding("question_mark", "help", "Help"),
    ]

    def __init__(self, manager: Manager) -> None:
        super().__init__()
        self._manager = manager
        self._sessions: list[dict] = []
        self._repos: dict = {}
        self._expanded: set[str] = set()  # session row keys that are expanded

    def compose(self) -> ComposeResult:
        yield DataTable(id="session-table", cursor_type="row", zebra_stripes=False)
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one(DataTable)
        table.add_columns("Session", "Repo", "Base Branch", "Windows", "")
        self._refresh_table()

    def on_screen_resume(self) -> None:
        self._refresh_table()

    def _refresh_table(self) -> None:
        self._repos = {r.id: r for r in get_repos(self._manager._conn)}
        self._sessions = self._manager.list_sessions()

        table = self.query_one(DataTable)
        table.clear()

        for session in self._sessions:
            repo = self._repos.get(session["repo_id"]) if session["repo_id"] else None
            repo_name = repo.name if repo else "-"
            base_branch = session["base_branch"] or "-"
            windows = str(session["windows"]) if session["windows"] is not None else "-"
            status = ""
            if session["attached"]:
                status = "[green]●[/green]"
            elif session["live"]:
                status = "[blue]○[/blue]"
            if not session["managed"]:
                status += " [dim]unmanaged[/dim]"
            row_key = str(session["id"]) if session["id"] is not None else f"unmanaged:{session['name']}"
            expanded = row_key in self._expanded
            can_expand = session["live"]
            indicator = "▾" if expanded else "▸" if can_expand else " "
            table.add_row(
                f"{indicator} {session['name']}",
                _truncate(repo_name, 30),
                _truncate(base_branch, 30),
                windows,
                status,
                key=row_key,
            )

            if expanded and session["live"]:
                tmux_windows = tmux.list_windows(session["name"])
                # Build worktree lookup by path for this session
                wt_by_path: dict[str, str] = {}
                if session["managed"] and session["id"] is not None:
                    for wt in get_worktrees_for_session(self._manager._conn, session["id"]):
                        wt_by_path[wt.path] = wt.branch
                for i, win in enumerate(tmux_windows):
                    is_last = i == len(tmux_windows) - 1
                    prefix = "└" if is_last else "├"
                    wt_branch = wt_by_path.get(win["path"])
                    cmd = win.get("command", "")
                    # Claude shows up as a version number (e.g. 2.1.89)
                    is_claude = bool(cmd and re.match(r"^\d+\.\d+\.\d+", cmd))
                    if is_claude:
                        cmd_display = "[#E87B35]✦ claude[/#E87B35]"
                    elif cmd and cmd != "zsh":
                        cmd_display = f"[italic]{cmd}[/italic]"
                    else:
                        cmd_display = ""
                    col_cmd = cmd_display
                    col_detail = ""
                    if wt_branch:
                        col_detail = f"[dim]wt:[/dim] {wt_branch}"
                    else:
                        col_detail = f"[dim]{_truncate(win['path'], 30)}[/dim]"
                    table.add_row(
                        f"  [dim]{prefix}[/dim] [dim]{win['name']}[/dim]",
                        col_cmd,
                        col_detail,
                        "",
                        "",
                        key=f"win:{session['name']}:{win['index']}",
                    )

        if self._sessions:
            table.move_cursor(row=0)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        self._switch_to_session(event.row_key.value)

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()

    def _current_row_key(self) -> str | None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return None
        return table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value

    def _is_child_row(self, row_key: str) -> bool:
        return row_key.startswith("wt:") or row_key.startswith("win:")

    def action_select(self) -> None:
        row_key = self._current_row_key()
        if row_key is None:
            return
        if row_key.startswith("win:"):
            parts = row_key.split(":", 2)
            session_name = parts[1]
            window_index = int(parts[2])
            _write_switch({"type": "window", "session": session_name, "window": window_index})
            self.app.exit()
            return
        if self._is_child_row(row_key):
            return
        self._switch_to_session(row_key)

    def action_toggle_expand(self) -> None:
        row_key = self._current_row_key()
        if row_key is None or self._is_child_row(row_key):
            return
        session = self._session_for_row_key(row_key)
        if session is None or not session["live"]:
            return
        if row_key in self._expanded:
            self._expanded.discard(row_key)
        else:
            self._expanded.add(row_key)
        self._refresh_table()

    def _switch_to_session(self, row_key: str | None) -> None:
        if row_key is None:
            return
        session = self._session_for_row_key(row_key)
        if session is None:
            return
        _write_switch({"type": "session", "target": session["name"]})
        self.app.exit()

    def _session_for_row_key(self, row_key: str) -> dict | None:
        if row_key.startswith("unmanaged:"):
            name = row_key[len("unmanaged:"):]
            return next((s for s in self._sessions if s["name"] == name), None)
        session_id = int(row_key)
        return next((s for s in self._sessions if s["id"] == session_id), None)

    def action_quit(self) -> None:
        self.app.exit()

    def action_new_session(self) -> None:
        self.app.push_screen(NewSessionScreen(self._manager))

    def _current_session(self) -> dict | None:
        """Get the session for the current row, or None if on a worktree row."""
        row_key = self._current_row_key()
        if row_key is None or self._is_child_row(row_key):
            return None
        return self._session_for_row_key(row_key)

    def action_new_tab(self) -> None:
        session = self._current_session()
        if session is None or not session["managed"]:
            return
        self.app.push_screen(NewTabScreen(self._manager, session["id"], session["name"]))

    def action_rename(self) -> None:
        row_key = self._current_row_key()
        if row_key is None:
            return
        # Rename a tmux window (tab)
        if row_key.startswith("win:"):
            # key format: win:<session_name>:<index>
            parts = row_key.split(":", 2)
            session_name = parts[1]
            window_index = int(parts[2])
            # Get current window name from tmux
            windows = tmux.list_windows(session_name)
            win = next((w for w in windows if w["index"] == window_index), None)
            if win is None:
                return
            self.app.push_screen(RenameWindowScreen(session_name, window_index, win["name"]))
            return
        # Rename a session
        session = self._current_session()
        if session is None or not session["managed"]:
            return
        self.app.push_screen(RenameSessionScreen(self._manager, session["id"], session["name"]))

    def action_edit_branch(self) -> None:
        session = self._current_session()
        if session is None or not session["managed"]:
            return
        self.app.push_screen(EditBranchScreen(self._manager, session["id"], session["name"]))

    def action_adopt(self) -> None:
        session = self._current_session()
        if session is None or session["managed"]:
            return
        self.app.push_screen(AdoptSessionScreen(self._manager, session["name"]))

    def action_delete_session(self) -> None:
        session = self._current_session()
        if session is None:
            return

        if session["managed"]:
            name = session["name"]
            msg = "Remove from torchard."
            if session["live"]:
                msg += " The tmux session will also be killed."

            def on_confirm(confirmed: bool) -> None:
                if not confirmed:
                    return
                self._manager.delete_session(session["id"], cleanup_worktrees=False)
                self._refresh_table()

            self.app.push_screen(
                ConfirmModal(f"Delete session '{name}'?", msg),
                on_confirm,
            )
        else:
            # Unmanaged - just offer to kill the tmux session
            name = session["name"]

            def on_confirm_kill(confirmed: bool) -> None:
                if not confirmed:
                    return
                try:
                    tmux.kill_session(name)
                except tmux.TmuxError:
                    pass
                self._refresh_table()

            self.app.push_screen(
                ConfirmModal(f"Kill tmux session '{name}'?", "This will close all windows in the session."),
                on_confirm_kill,
            )

    def action_cleanup(self) -> None:
        self.app.push_screen(CleanupScreen(self._manager))

    def action_help(self) -> None:
        self.app.push_screen(HelpScreen())


_SWITCH_FILE = Path(tempfile.gettempdir()) / "torchard-switch.json"


def _write_switch(action: dict) -> None:
    _SWITCH_FILE.write_text(json.dumps(action))


def _truncate(text: str, max_len: int) -> str:
    if len(text) <= max_len:
        return text
    return text[: max_len - 1] + "…"
