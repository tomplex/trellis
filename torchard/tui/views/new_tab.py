"""New tab (worktree) creation screen."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import Footer, Input, Label, Static
from textual.containers import Vertical

import subprocess

from torchard.core.manager import Manager
from torchard.tui.views.session_list import _write_switch


class NewTabScreen(Screen):
    """Prompt for a branch name and create a new worktree tab in the given session."""

    BINDINGS = [
        Binding("escape", "cancel", "Cancel"),
    ]

    def __init__(self, manager: Manager, session_id: int, session_name: str) -> None:
        super().__init__()
        self._manager = manager
        self._session_id = session_id
        self._session_name = session_name

    def compose(self) -> ComposeResult:
        yield Vertical(
            Static(
                f"[bold]New Tab[/bold] — session [accent]{self._session_name}[/accent]",
                id="new-tab-title",
            ),
            Label("Branch name:", id="new-tab-label"),
            Input(placeholder="e.g. my-feature", id="new-tab-input"),
            Static("", id="new-tab-error"),
            Static(
                "[dim]Enter[/dim] to create  [dim]Escape[/dim] to cancel",
                id="new-tab-hint",
            ),
            id="new-tab-container",
        )
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#new-tab-input", Input).focus()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        self._submit(event.value.strip())

    def _submit(self, branch_name: str) -> None:
        error_widget = self.query_one("#new-tab-error", Static)

        if not branch_name:
            error_widget.update("[red]Branch name cannot be empty.[/red]")
            return

        try:
            self._manager.add_tab(self._session_id, branch_name)
        except Exception as exc:
            error_widget.update(f"[red]{exc}[/red]")
            return

        # Launch claude in the new window
        subprocess.run(["tmux", "send-keys", "-t", f"{self._session_name}:{branch_name}", "claude", "Enter"])
        _write_switch({"type": "session", "target": self._session_name})
        self.app.exit()

    def action_cancel(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    NewTabScreen {
        align: center middle;
    }
    #new-tab-container {
        width: 70;
        height: auto;
        padding: 2 4;
        border: round $accent;
    }
    #new-tab-title {
        text-align: center;
        margin-bottom: 1;
    }
    #new-tab-label {
        margin-bottom: 0;
    }
    #new-tab-input {
        margin-bottom: 1;
    }
    #new-tab-error {
        color: $error;
        margin-bottom: 1;
        min-height: 1;
    }
    #new-tab-hint {
        text-align: center;
        color: $text-muted;
        margin-top: 1;
    }
    """
