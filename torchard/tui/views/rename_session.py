"""Rename session screen."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import Footer, Input, Static

from torchard.core import tmux
from torchard.core.db import get_session_by_name
from torchard.core.manager import Manager


class RenameSessionScreen(Screen):
    """Rename a managed session."""

    BINDINGS = [
        Binding("escape", "cancel", "Cancel"),
    ]

    def __init__(self, manager: Manager, session_id: int, current_name: str) -> None:
        super().__init__()
        self._manager = manager
        self._session_id = session_id
        self._current_name = current_name

    def compose(self) -> ComposeResult:
        with Vertical(id="rename-container"):
            yield Static(
                f"[bold]Rename Session[/bold] — [dim]{self._current_name}[/dim]",
                id="rename-title",
            )
            yield Input(value=self._current_name, id="rename-input")
            yield Static("", id="rename-error")
            yield Static(
                "[dim]Enter[/dim] to rename  [dim]Escape[/dim] to cancel",
                id="rename-hint",
            )
        yield Footer()

    def on_mount(self) -> None:
        inp = self.query_one("#rename-input", Input)
        inp.cursor_position = len(self._current_name)
        inp.focus()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        name = tmux.sanitize_session_name(event.value.strip())
        error = self.query_one("#rename-error", Static)

        if not name:
            error.update("[red]Name cannot be empty.[/red]")
            return
        if name == self._current_name:
            self.app.pop_screen()
            return
        existing = get_session_by_name(self._manager._conn, name)
        if existing is not None:
            error.update(f"[red]Session '{name}' already exists.[/red]")
            return
        try:
            self._manager.rename_session(self._session_id, name)
        except Exception as exc:
            error.update(f"[red]{exc}[/red]")
            return
        self.app.pop_screen()

    def action_cancel(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    RenameSessionScreen {
        align: center middle;
    }
    #rename-container {
        width: 70;
        height: auto;
        padding: 2 4;
        border: solid $accent;
    }
    #rename-title {
        text-align: center;
        margin-bottom: 1;
    }
    #rename-input {
        margin-bottom: 1;
    }
    #rename-error {
        color: $error;
        margin-bottom: 1;
        min-height: 1;
    }
    #rename-hint {
        text-align: center;
        color: $text-muted;
    }
    """


class RenameWindowScreen(Screen):
    """Rename a tmux window (tab)."""

    BINDINGS = [
        Binding("escape", "cancel", "Cancel"),
    ]

    def __init__(self, session_name: str, window_index: int, current_name: str) -> None:
        super().__init__()
        self._session_name = session_name
        self._window_index = window_index
        self._current_name = current_name

    def compose(self) -> ComposeResult:
        with Vertical(id="rename-container"):
            yield Static(
                f"[bold]Rename Tab[/bold] — [dim]{self._session_name}:{self._current_name}[/dim]",
                id="rename-title",
            )
            yield Input(value=self._current_name, id="rename-input")
            yield Static("", id="rename-error")
            yield Static(
                "[dim]Enter[/dim] to rename  [dim]Escape[/dim] to cancel",
                id="rename-hint",
            )
        yield Footer()

    def on_mount(self) -> None:
        inp = self.query_one("#rename-input", Input)
        inp.cursor_position = len(self._current_name)
        inp.focus()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        name = event.value.strip()
        error = self.query_one("#rename-error", Static)

        if not name:
            error.update("[red]Name cannot be empty.[/red]")
            return
        if name == self._current_name:
            self.app.pop_screen()
            return
        try:
            tmux.rename_window(self._session_name, self._window_index, name)
        except Exception as exc:
            error.update(f"[red]{exc}[/red]")
            return
        self.app.pop_screen()

    def action_cancel(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = RenameSessionScreen.DEFAULT_CSS
