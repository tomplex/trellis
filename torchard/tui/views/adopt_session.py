"""Adopt an unmanaged tmux session — associate it with a repo and base branch."""

from __future__ import annotations

from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import Footer, Input, Label, ListItem, ListView, Static

from torchard.core.db import get_repos
from torchard.core.git import GitError, list_branches
from torchard.core.manager import Manager
from torchard.core.models import Repo


class AdoptSessionScreen(Screen):
    """Two-step wizard: pick a repo, pick a base branch, then adopt the session."""

    BINDINGS = [
        Binding("escape", "go_back", "Back", show=True),
    ]

    DEFAULT_CSS = """
    AdoptSessionScreen {
        align: center middle;
    }
    #adopt-container {
        width: 70;
        height: auto;
        max-height: 40;
        border: solid $accent;
        padding: 1 2;
    }
    #adopt-title {
        text-align: center;
        color: $accent;
        margin-bottom: 1;
        text-style: bold;
    }
    #adopt-hint {
        color: $text-muted;
        margin-bottom: 1;
        text-align: center;
    }
    #adopt-filter {
        margin-bottom: 1;
    }
    #adopt-list {
        height: 12;
        border: solid $panel;
    }
    #adopt-error {
        color: $error;
        margin-top: 1;
        text-align: center;
    }
    """

    def __init__(self, manager: Manager, session_name: str) -> None:
        super().__init__()
        self._manager = manager
        self._session_name = session_name
        self._step = 1  # 1 = repo, 2 = branch
        self._selected_repo: Repo | None = None
        self._repos: list[Repo] = []
        self._branches: list[str] = []
        self._awaiting_repo_path = False

    def compose(self) -> ComposeResult:
        with Vertical(id="adopt-container"):
            yield Static("", id="adopt-title")
            yield Static("", id="adopt-hint")
            yield Input(placeholder="Filter…", id="adopt-filter")
            yield ListView(id="adopt-list")
            yield Static("", id="adopt-error")
        yield Footer()

    def on_mount(self) -> None:
        self._render_step()

    def _set_title(self, text: str) -> None:
        self.query_one("#adopt-title", Static).update(text)

    def _set_hint(self, text: str) -> None:
        self.query_one("#adopt-hint", Static).update(text)

    def _set_error(self, text: str) -> None:
        self.query_one("#adopt-error", Static).update(text)

    def _render_step(self) -> None:
        self._awaiting_repo_path = False
        self._set_error("")
        if self._step == 1:
            self._render_repo_step()
        elif self._step == 2:
            self._render_branch_step()

    def _render_repo_step(self) -> None:
        self._set_title(f"Adopt '{self._session_name}' — Select Repository")
        self._set_hint("Type to filter. Enter to select.")
        fi = self.query_one("#adopt-filter", Input)
        fi.placeholder = "Filter repos…"
        fi.value = ""
        self._repos = get_repos(self._manager._conn)
        self._populate_repo_list(self._repos)
        fi.focus()

    def _render_branch_step(self) -> None:
        assert self._selected_repo is not None
        self._set_title(f"Adopt '{self._session_name}' — Select Base Branch  [dim]({self._selected_repo.name})[/dim]")
        self._set_hint("Pick the branch new worktrees will branch from.")
        fi = self.query_one("#adopt-filter", Input)
        fi.placeholder = "Filter or type a new branch…"
        fi.value = ""
        try:
            self._branches = list_branches(self._selected_repo.path)
        except GitError as exc:
            self._branches = []
            self._set_error(str(exc))
        self._populate_branch_list(self._branches, "")
        fi.focus()

    def _populate_repo_list(self, repos: list[Repo]) -> None:
        lv = self.query_one("#adopt-list", ListView)
        lv.clear()
        for repo in repos:
            lv.append(ListItem(Label(f"[bold]{repo.name}[/bold]  [dim]{repo.path}[/dim]"), id=f"repo-{repo.id}"))
        lv.append(ListItem(Label("[green]+ Add new repo path…[/green]"), id="add-repo"))

    def _populate_branch_list(self, branches: list[str], query: str) -> None:
        lv = self.query_one("#adopt-list", ListView)
        lv.clear()
        for branch in branches:
            lv.append(ListItem(Label(branch), id=f"branch-{branch}"))
        if query and query not in branches:
            lv.append(ListItem(Label(f"[green]+ New branch: [bold]{query}[/bold][/green]"), id="new-branch"))

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id != "adopt-filter" or self._awaiting_repo_path:
            return
        query = event.value.lower()
        if self._step == 1:
            filtered = [r for r in self._repos if query in r.name.lower() or query in r.path.lower()]
            self._populate_repo_list(filtered)
        elif self._step == 2:
            filtered = [b for b in self._branches if query in b.lower()]
            self._populate_branch_list(filtered, event.value)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id != "adopt-filter":
            return
        if self._awaiting_repo_path:
            self._finish_add_repo(event.value)
        else:
            self._confirm_list_selection(typed_value=event.value)

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        item_id = event.item.id
        if self._step == 1:
            self._select_repo_item(item_id)
        elif self._step == 2:
            self._select_branch_item(item_id)

    def _confirm_list_selection(self, typed_value: str) -> None:
        lv = self.query_one("#adopt-list", ListView)
        highlighted = lv.highlighted_child
        if highlighted is not None:
            item_id = highlighted.id
            if self._step == 1:
                self._select_repo_item(item_id)
            elif self._step == 2:
                self._select_branch_item(item_id)
        elif self._step == 2 and typed_value:
            self._adopt(typed_value)

    def _select_repo_item(self, item_id: str | None) -> None:
        if item_id == "add-repo":
            self._enter_repo_path_mode()
            return
        if item_id and item_id.startswith("repo-"):
            repo_id = int(item_id[len("repo-"):])
            repo = next((r for r in self._repos if r.id == repo_id), None)
            if repo is not None:
                self._selected_repo = repo
                self._step = 2
                self._render_step()

    def _select_branch_item(self, item_id: str | None) -> None:
        if item_id == "new-branch":
            typed = self.query_one("#adopt-filter", Input).value
            if typed:
                self._adopt(typed)
            return
        if item_id and item_id.startswith("branch-"):
            branch = item_id[len("branch-"):]
            self._adopt(branch)

    def _adopt(self, base_branch: str) -> None:
        assert self._selected_repo is not None
        try:
            self._manager.adopt_session(
                session_name=self._session_name,
                repo_path=self._selected_repo.path,
                base_branch=base_branch,
            )
        except Exception as exc:
            self._set_error(f"Error: {exc}")
            return
        self.app.pop_screen()

    def _enter_repo_path_mode(self) -> None:
        self._awaiting_repo_path = True
        self._set_hint("Enter the full path to a git repo, then press Enter.")
        fi = self.query_one("#adopt-filter", Input)
        fi.placeholder = "e.g. /home/you/dev/myproject…"
        fi.value = ""
        fi.focus()

    def _finish_add_repo(self, path_str: str) -> None:
        self._awaiting_repo_path = False
        path = Path(path_str.strip()).expanduser().resolve()
        if not path.is_dir() or not (path / ".git").exists():
            self._set_error(f"'{path}' is not a git repository.")
            fi = self.query_one("#adopt-filter", Input)
            fi.placeholder = "Filter repos…"
            fi.value = ""
            self._set_hint("Type to filter. Enter to select.")
            return

        class _AdhocRepo:
            def __init__(self, p: Path) -> None:
                self.path = str(p)
                self.name = p.name
                self.id = None

        self._selected_repo = _AdhocRepo(path)  # type: ignore[assignment]
        self._step = 2
        self._render_step()

    def action_go_back(self) -> None:
        if self._awaiting_repo_path:
            self._awaiting_repo_path = False
            fi = self.query_one("#adopt-filter", Input)
            fi.placeholder = "Filter repos…"
            fi.value = ""
            self._populate_repo_list(self._repos)
            self._set_hint("Type to filter. Enter to select.")
            self._set_error("")
            return
        if self._step == 1:
            self.app.pop_screen()
        else:
            self._step = 1
            self._selected_repo = None
            self._render_step()
