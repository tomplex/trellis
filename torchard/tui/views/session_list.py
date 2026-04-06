"""Main session list screen."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static
from textual.containers import Vertical

from torchard.core import tmux
from torchard.core.db import get_repos
from torchard.core.manager import Manager
from torchard.tui.views.adopt_session import AdoptSessionScreen
from torchard.tui.views.cleanup import CleanupScreen
from torchard.tui.views.confirm import ConfirmModal
from torchard.tui.views.edit_branch import EditBranchScreen
from torchard.tui.views.new_session import NewSessionScreen
from torchard.tui.views.new_tab import NewTabScreen
from torchard.tui.views.rename_session import RenameSessionScreen


class PlaceholderScreen(Screen):
    """Generic not-yet-implemented placeholder screen."""

    BINDINGS = [Binding("escape", "dismiss", "Back")]

    def __init__(self, title: str) -> None:
        super().__init__()
        self._title = title

    def compose(self) -> ComposeResult:
        yield Vertical(
            Static(f"[bold]{self._title}[/bold]", id="placeholder-title"),
            Static("Not implemented yet", id="placeholder-msg"),
            Static("Press [bold]Escape[/bold] to go back", id="placeholder-hint"),
            id="placeholder-container",
        )

    def action_dismiss(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    PlaceholderScreen {
        align: center middle;
    }
    #placeholder-container {
        width: auto;
        height: auto;
        align: center middle;
        padding: 2 4;
    }
    #placeholder-title {
        text-align: center;
        color: $accent;
        margin-bottom: 1;
    }
    #placeholder-msg {
        text-align: center;
    }
    #placeholder-hint {
        text-align: center;
        color: $text-muted;
        margin-top: 1;
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
            table.add_row(
                session["name"],
                _truncate(repo_name, 30),
                _truncate(base_branch, 30),
                windows,
                status,
                key=row_key,
            )

        if self._sessions:
            table.move_cursor(row=0)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        self._switch_to_session(event.row_key.value)

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()

    def action_select(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key
        self._switch_to_session(row_key.value)

    def _switch_to_session(self, row_key: str | None) -> None:
        if row_key is None:
            return
        session = self._session_for_row_key(row_key)
        if session is None:
            return
        try:
            tmux.switch_client(session["name"])
        except tmux.TmuxError:
            pass
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

    def action_new_tab(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        session = self._session_for_row_key(row_key)
        if session is None or not session["managed"]:
            return
        self.app.push_screen(NewTabScreen(self._manager, session["id"], session["name"]))

    def action_rename(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        session = self._session_for_row_key(row_key)
        if session is None or not session["managed"]:
            return
        self.app.push_screen(RenameSessionScreen(self._manager, session["id"], session["name"]))

    def action_edit_branch(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        session = self._session_for_row_key(row_key)
        if session is None or not session["managed"]:
            return
        self.app.push_screen(EditBranchScreen(self._manager, session["id"], session["name"]))

    def action_adopt(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        session = self._session_for_row_key(row_key)
        if session is None or session["managed"]:
            return
        self.app.push_screen(AdoptSessionScreen(self._manager, session["name"]))

    def action_delete_session(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        session = self._session_for_row_key(row_key)
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
        self.app.push_screen(PlaceholderScreen("Help"))


def _truncate(text: str, max_len: int) -> str:
    if len(text) <= max_len:
        return text
    return text[: max_len - 1] + "…"
