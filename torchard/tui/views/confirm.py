"""Reusable confirmation modal."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Static


class ConfirmModal(ModalScreen[bool]):
    """Generic yes/no confirmation modal."""

    BINDINGS = [
        Binding("y", "confirm", "Yes"),
        Binding("n", "cancel", "No"),
        Binding("escape", "cancel", "No"),
    ]

    def __init__(self, title: str, body: str) -> None:
        super().__init__()
        self._title = title
        self._body = body

    def compose(self) -> ComposeResult:
        yield Vertical(
            Static(f"[bold]{self._title}[/bold]", id="confirm-title"),
            Static(self._body, id="confirm-body"),
            Horizontal(
                Static("[bold green]\\[y][/bold green] Yes", id="confirm-yes"),
                Static("[bold red]\\[n][/bold red] No", id="confirm-no"),
                id="confirm-buttons",
            ),
            id="confirm-container",
        )

    def action_confirm(self) -> None:
        self.dismiss(True)

    def action_cancel(self) -> None:
        self.dismiss(False)

    DEFAULT_CSS = """
    ConfirmModal {
        align: center middle;
    }
    #confirm-container {
        background: #16213e;
        border: solid #00aaff;
        padding: 2 4;
        width: 70;
        height: auto;
    }
    #confirm-title {
        text-align: center;
        color: #ff6b6b;
        margin-bottom: 1;
    }
    #confirm-body {
        text-align: center;
        color: #e0e0e0;
        margin-bottom: 2;
    }
    #confirm-buttons {
        align: center middle;
        height: auto;
    }
    #confirm-yes {
        margin-right: 4;
    }
    #confirm-no {
        margin-left: 4;
    }
    """
