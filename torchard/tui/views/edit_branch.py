"""Edit a session's base branch."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import Footer, Input, Label, ListItem, ListView, Static

from torchard.core.db import get_repos
from torchard.core.git import GitError, list_branches
from torchard.core.manager import Manager
from torchard.tui.utils import safe_id


class EditBranchScreen(Screen):
    """Pick a new base branch for a managed session."""

    BINDINGS = [
        Binding("escape", "cancel", "Cancel"),
    ]

    DEFAULT_CSS = """
    EditBranchScreen {
        align: center middle;
    }
    #editbranch-container {
        width: 90;
        height: auto;
        max-height: 50;
        border: solid $accent;
        padding: 1 2;
    }
    #editbranch-title {
        text-align: center;
        color: $accent;
        margin-bottom: 1;
        text-style: bold;
    }
    #editbranch-hint {
        color: $text-muted;
        margin-bottom: 1;
        text-align: center;
    }
    #editbranch-filter {
        margin-bottom: 1;
    }
    #editbranch-list {
        height: 18;
        border: solid $panel;
    }
    #editbranch-error {
        color: $error;
        margin-top: 1;
        text-align: center;
    }
    """

    def __init__(self, manager: Manager, session_id: int, session_name: str) -> None:
        super().__init__()
        self._manager = manager
        self._session_id = session_id
        self._session_name = session_name
        self._branches: list[str] = []
        self._render_seq = 0
        self._id_to_branch: dict[str, str] = {}

    def compose(self) -> ComposeResult:
        with Vertical(id="editbranch-container"):
            yield Static("", id="editbranch-title")
            yield Static("", id="editbranch-hint")
            yield Input(placeholder="Filter or type a branch…", id="editbranch-filter")
            yield ListView(id="editbranch-list")
            yield Static("", id="editbranch-error")
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#editbranch-title", Static).update(
            f"Edit Branch — [dim]{self._session_name}[/dim]"
        )
        self.query_one("#editbranch-hint", Static).update(
            "Pick the branch new worktrees will branch from."
        )

        # Find the repo for this session
        from torchard.core.db import get_sessions
        sessions = get_sessions(self._manager._conn)
        session = next((s for s in sessions if s.id == self._session_id), None)
        if session is None:
            self.query_one("#editbranch-error", Static).update("[red]Session not found[/red]")
            return

        repos = {r.id: r for r in get_repos(self._manager._conn)}
        repo = repos.get(session.repo_id)
        if repo is None:
            self.query_one("#editbranch-error", Static).update("[red]Repo not found[/red]")
            return

        try:
            self._branches = list_branches(repo.path)
        except GitError as exc:
            self._branches = []
            self.query_one("#editbranch-error", Static).update(str(exc))

        self._populate(self._branches, "")
        self.query_one("#editbranch-filter", Input).focus()

    def _populate(self, branches: list[str], query: str) -> None:
        self._render_seq += 1
        seq = self._render_seq
        self._id_to_branch.clear()
        lv = self.query_one("#editbranch-list", ListView)
        lv.clear()
        for branch in branches:
            widget_id = f"branch-{safe_id(branch)}-{seq}"
            self._id_to_branch[widget_id] = branch
            lv.append(ListItem(Label(branch), id=widget_id))
        if query and query not in branches:
            lv.append(ListItem(
                Label(f"[green]+ Use: [bold]{query}[/bold][/green]"),
                id=f"new-branch-{seq}",
            ))

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id != "editbranch-filter":
            return
        query = event.value.lower()
        filtered = [b for b in self._branches if query in b.lower()]
        self._populate(filtered, event.value)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id != "editbranch-filter":
            return
        lv = self.query_one("#editbranch-list", ListView)
        highlighted = lv.highlighted_child
        if highlighted is None and len(lv.children) > 0:
            highlighted = lv.children[0]
        if highlighted is not None:
            self._select_item(highlighted.id)
        elif event.value.strip():
            self._apply(event.value.strip())

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        self._select_item(event.item.id)

    def _select_item(self, item_id: str | None) -> None:
        if item_id and item_id.startswith("new-branch"):
            typed = self.query_one("#editbranch-filter", Input).value.strip()
            if typed:
                self._apply(typed)
            return
        if item_id and item_id in self._id_to_branch:
            self._apply(self._id_to_branch[item_id])

    def _apply(self, branch: str) -> None:
        try:
            self._manager.set_base_branch(self._session_id, branch)
        except ValueError as exc:
            self.query_one("#editbranch-error", Static).update(f"[red]{exc}[/red]")
            return
        self.app.pop_screen()

    def action_cancel(self) -> None:
        self.app.pop_screen()
