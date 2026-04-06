"""Checkout a PR or branch into a worktree and launch claude."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import Footer, Input, Static

from torchard.core.manager import Manager


class ReviewScreen(Screen):
    """Enter a PR number or branch name to check out and review."""

    BINDINGS = [
        Binding("escape", "cancel", "Cancel"),
    ]

    def __init__(self, manager: Manager, repo_path: str, repo_name: str) -> None:
        super().__init__()
        self._manager = manager
        self._repo_path = repo_path
        self._repo_name = repo_name

    def compose(self) -> ComposeResult:
        with Vertical(id="review-container"):
            yield Static(
                f"[bold]Review[/bold] — [dim]{self._repo_name}[/dim]",
                id="review-title",
            )
            yield Static(
                "Enter a PR number or branch name",
                id="review-hint",
            )
            yield Input(placeholder="e.g. 1234 or feat/my-branch", id="review-input")
            yield Static("", id="review-error")
            yield Static(
                "[dim]Enter[/dim] to checkout  [dim]Escape[/dim] to cancel",
                id="review-footer-hint",
            )
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#review-input", Input).focus()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        value = event.value.strip()
        error = self.query_one("#review-error", Static)

        if not value:
            error.update("[red]Please enter a PR number or branch name.[/red]")
            return

        error.update("[dim]Checking out...[/dim]")
        self.run_worker(lambda: self._do_checkout(value), thread=True)

    def _do_checkout(self, pr_or_branch: str) -> None:
        try:
            session, worktree_path = self._manager.checkout_and_review(
                self._repo_path, pr_or_branch,
            )
        except Exception as exc:
            self.call_from_thread(
                self.query_one("#review-error", Static).update,
                f"[red]{exc}[/red]",
            )
            return

        # Launch claude in the new session's first window
        import subprocess
        subprocess.run([
            "tmux", "send-keys", "-t", session.name,
            "claude", "Enter",
        ])

        # Write switch file and exit
        from torchard.tui.switch import write_switch
        write_switch({"type": "session", "target": session.name})
        self.call_from_thread(self.app.exit)

    def action_cancel(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    ReviewScreen {
        align: center middle;
    }
    #review-container {
        width: 70;
        height: auto;
        padding: 2 4;
        border: solid $accent;
    }
    #review-title {
        text-align: center;
        margin-bottom: 1;
    }
    #review-hint {
        text-align: center;
        color: $text-muted;
        margin-bottom: 1;
    }
    #review-input {
        margin-bottom: 1;
    }
    #review-error {
        color: $error;
        margin-bottom: 1;
        min-height: 1;
    }
    #review-footer-hint {
        text-align: center;
        color: $text-muted;
    }
    """
