"""Main session list screen."""

from __future__ import annotations

import re
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Input, Static
from textual.containers import Vertical

from torchard.core import tmux
from torchard.core.db import get_repos, get_worktrees_for_session
from torchard.core.manager import Manager
from torchard.tui.switch import write_switch
from torchard.tui.views.adopt_session import AdoptSessionScreen
from torchard.tui.views.cleanup import CleanupScreen
from torchard.tui.views.confirm import ConfirmModal
from torchard.tui.views.edit_branch import EditBranchScreen
from torchard.tui.views.history import HistoryScreen
from torchard.tui.views.new_session import NewSessionScreen
from torchard.tui.views.new_tab import NewTabScreen
from torchard.tui.views.rename_session import RenameSessionScreen, RenameWindowScreen
from torchard.tui.views.review import ReviewScreen


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
  [#00aaff]g[/#00aaff]         Launch claude in session
  [#00aaff]p[/#00aaff]         Checkout PR/branch + claude
  [#00aaff]h[/#00aaff]         Conversation history
  [#00aaff]a[/#00aaff]         Adopt unmanaged session
  [#00aaff]c[/#00aaff]         Cleanup stale worktrees
  [#00aaff]j/k[/#00aaff]       Navigate up/down
  [#00aaff]/[/#00aaff]         Filter sessions
  [#00aaff]x[/#00aaff]         Kill tab (on expanded tab)
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
        Binding("escape", "escape_pressed", "Quit", show=False),
        Binding("slash", "start_filter", "/Filter", show=True),
        Binding("j,down", "cursor_down", "Down", show=False),
        Binding("k,up", "cursor_up", "Up", show=False),
        Binding("enter", "select", "Switch"),
        Binding("tab", "toggle_expand", "Expand"),
        Binding("n", "new_session", "New"),
        Binding("w", "new_tab", "Tab"),
        Binding("x", "kill_window", "Kill tab", show=False),
        Binding("d", "delete_session", "Delete"),
        Binding("r", "rename", "Rename"),
        Binding("b", "edit_branch", "Branch"),
        Binding("g", "launch_claude", "Claude"),
        Binding("p", "review", "PR/Branch"),
        Binding("a", "adopt", "Adopt"),
        Binding("h", "history", "History"),
        Binding("c", "cleanup", "Cleanup"),
        Binding("question_mark", "help", "Help"),
    ]

    def __init__(self, manager: Manager) -> None:
        super().__init__()
        self._manager = manager
        self._sessions: list[dict] = []
        self._repos: dict = {}
        self._expanded: set[str] = set()  # session row keys that are expanded
        self._filter: str = ""

    def compose(self) -> ComposeResult:
        yield Input(placeholder="Type to filter…", id="session-filter", classes="hidden")
        yield DataTable(id="session-table", cursor_type="row", zebra_stripes=False)
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one(DataTable)
        table.add_columns("Session", "Repo", "Base Branch", "Windows", "")
        self._refresh_table()
        table.focus()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "session-filter":
            self._filter = event.value.lower()
            self._refresh_table()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "session-filter":
            # Dismiss filter, keep the filter text active
            self._dismiss_filter()

    def action_start_filter(self) -> None:
        fi = self.query_one("#session-filter", Input)
        fi.remove_class("hidden")
        fi.value = ""
        fi.focus()

    def _dismiss_filter(self) -> None:
        fi = self.query_one("#session-filter", Input)
        if not fi.value:
            fi.add_class("hidden")
            self._filter = ""
            self._refresh_table()
        self.query_one(DataTable).focus()

    def action_escape_pressed(self) -> None:
        fi = self.query_one("#session-filter", Input)
        if fi.has_focus or (not fi.has_class("hidden") and fi.value):
            fi.value = ""
            fi.add_class("hidden")
            self._filter = ""
            self._refresh_table()
            self.query_one(DataTable).focus()
            return
        self.app.exit()

    def on_screen_resume(self) -> None:
        self._refresh_table()

    def _refresh_table(self, restore_key: str | None = None) -> None:
        self._repos = {r.id: r for r in get_repos(self._manager._conn)}
        self._sessions = self._manager.list_sessions()

        # Sort: "main" always first, then attached, then live, then dead
        self._sessions.sort(key=lambda s: (
            0 if s["name"] == "main" else 1,
            0 if s["attached"] else 1 if s["live"] else 2,
            s["name"].lower(),
        ))

        table = self.query_one(DataTable)
        table.clear()

        for session in self._sessions:
            # Filter
            if self._filter:
                name_match = self._filter in session["name"].lower()
                repo = self._repos.get(session["repo_id"]) if session["repo_id"] else None
                repo_match = repo and self._filter in repo.name.lower()
                branch_match = session["base_branch"] and self._filter in session["base_branch"].lower()
                if not (name_match or repo_match or branch_match):
                    continue
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
            # Restore cursor to the row with the given key, or row 0
            target_row = 0
            if restore_key is not None:
                for i, rk in enumerate(table.rows):
                    if rk.value == restore_key:
                        target_row = i
                        break
            table.move_cursor(row=target_row)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        row_key = event.row_key.value
        if row_key and row_key.startswith("win:"):
            parts = row_key.split(":", 2)
            write_switch({"type": "window", "session": parts[1], "window": int(parts[2])})
            self.app.exit()
            return
        if row_key and self._is_child_row(row_key):
            return
        self._switch_to_session(row_key)

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
            write_switch({"type": "window", "session": session_name, "window": window_index})
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
        self._refresh_table(restore_key=row_key)

    def _switch_to_session(self, row_key: str | None) -> None:
        if row_key is None:
            return
        session = self._session_for_row_key(row_key)
        if session is None:
            return
        write_switch({"type": "session", "target": session["name"]})
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

    def action_launch_claude(self) -> None:
        session = self._current_session()
        if session is None or not session["live"]:
            return
        # Create a new window in the session and send claude to it
        import subprocess
        subprocess.run(["tmux", "new-window", "-t", session["name"], "-n", "claude"])
        subprocess.run(["tmux", "send-keys", "-t", f"{session['name']}:claude", "claude", "Enter"])
        write_switch({"type": "session", "target": session["name"]})
        self.app.exit()

    def action_review(self) -> None:
        session = self._current_session()
        if session is None or not session["managed"]:
            return
        repo = self._repos.get(session["repo_id"])
        if repo is None:
            return
        self.app.push_screen(ReviewScreen(self._manager, repo.path, repo.name))

    def action_adopt(self) -> None:
        session = self._current_session()
        if session is None or session["managed"]:
            return
        self.app.push_screen(AdoptSessionScreen(self._manager, session["name"]))

    def action_kill_window(self) -> None:
        row_key = self._current_row_key()
        if row_key is None or not row_key.startswith("win:"):
            return
        parts = row_key.split(":", 2)
        session_name = parts[1]
        window_index = int(parts[2])

        def on_confirm(confirmed: bool) -> None:
            if not confirmed:
                return
            try:
                tmux.kill_window(session_name, window_index)
            except tmux.TmuxError:
                pass
            self._refresh_table()

        self.app.push_screen(
            ConfirmModal(f"Kill tab {window_index} in '{session_name}'?", "This will close the window and any processes in it."),
            on_confirm,
        )

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

    def action_history(self) -> None:
        session = self._current_session()
        scope_paths = None
        scope_label = None
        if session and session["managed"]:
            repo = self._repos.get(session["repo_id"])
            if repo:
                # Scope to repo path + all its worktree paths
                from torchard.core.db import get_worktrees_for_session
                paths = [repo.path]
                if session["id"] is not None:
                    for wt in get_worktrees_for_session(self._manager._conn, session["id"]):
                        paths.append(wt.path)
                # Also include worktrees root for this repo
                from pathlib import Path
                wt_root = str(Path.home() / "dev" / "worktrees" / repo.name)
                paths.append(wt_root)
                scope_paths = paths
                scope_label = session["name"]
        self.app.push_screen(HistoryScreen(self._manager, scope_paths, scope_label))

    def action_cleanup(self) -> None:
        self.app.push_screen(CleanupScreen(self._manager))

    def action_help(self) -> None:
        self.app.push_screen(HelpScreen())


def _get_claude_session_id(pane_pid: str) -> str | None:
    """Look up the Claude session UUID for a given pane PID."""
    if not pane_pid:
        return None
    pid_file = Path("/tmp/claude-sessions") / f"pid-{pane_pid}"
    try:
        return pid_file.read_text().strip() if pid_file.exists() else None
    except OSError:
        return None




def _truncate(text: str, max_len: int) -> str:
    if len(text) <= max_len:
        return text
    return text[: max_len - 1] + "…"
