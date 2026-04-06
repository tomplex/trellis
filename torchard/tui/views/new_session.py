"""New session creation wizard - multi-step screen."""

from __future__ import annotations

import re
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import Footer, Input, Label, ListItem, ListView, Static

from torchard.core import tmux
from torchard.core.db import get_session_by_name
from torchard.core.git import GitError, list_branches
from torchard.core.manager import Manager, detect_subsystems
from torchard.core.models import Repo


def _safe_id(text: str) -> str:
    """Sanitize a string for use as a textual widget ID."""
    return re.sub(r"[^a-zA-Z0-9_-]", "_", text)


def _sanitize_for_tmux(name: str) -> str:
    """Remove/replace characters not allowed in tmux session names."""
    # tmux session names can't contain dots or colons
    name = re.sub(r"[.:]", "-", name)
    name = name.strip(" -")
    return name or "new-session"


class NewSessionScreen(Screen):
    """Multi-step wizard for creating a new torchard session.

    Step 1 – pick a repo (or add one by path)
    Step 2 – pick / type a branch
    Step 3 – confirm / edit the session name
    """

    BINDINGS = [
        Binding("escape", "go_back", "Back", show=True),
    ]

    DEFAULT_CSS = """
    NewSessionScreen {
        align: center middle;
    }

    #wizard-container {
        width: 90;
        height: auto;
        max-height: 50;
        border: solid $accent;
        padding: 1 2;
    }

    #step-title {
        text-align: center;
        color: $accent;
        margin-bottom: 1;
        text-style: bold;
    }

    #step-hint {
        color: $text-muted;
        margin-bottom: 1;
        text-align: center;
    }

    #filter-input {
        margin-bottom: 1;
    }

    #item-list {
        height: 18;
        border: solid $panel;
    }

    #session-name-input {
        margin-top: 1;
    }

    #error-label {
        color: $error;
        margin-top: 1;
        text-align: center;
    }

    .hidden {
        display: none;
    }
    """

    def __init__(self, manager: Manager) -> None:
        super().__init__()
        self._manager = manager
        self._step = 1  # 1 = repo, 2 = branch, 3 = session name

        self._selected_repo: Repo | None = None
        self._selected_branch: str | None = None
        self._selected_subdirectory: str | None = None

        self._repos: list[Repo] = []
        self._branches: list[str] = []
        self._subsystems: list[str] = []

        # When True the filter input is being used to collect a raw filesystem path
        self._awaiting_repo_path = False
        self._render_seq = 0  # uniquify widget IDs across re-renders
        self._id_to_branch: dict[str, str] = {}  # widget id -> actual branch name
        self._id_to_subsystem: dict[str, str] = {}  # widget id -> subsystem path

    # ------------------------------------------------------------------
    # Compose
    # ------------------------------------------------------------------

    def compose(self) -> ComposeResult:
        with Vertical(id="wizard-container"):
            yield Static("", id="step-title")
            yield Static("", id="step-hint")
            yield Input(placeholder="Filter…", id="filter-input")
            yield ListView(id="item-list")
            yield Input(placeholder="Session name", id="session-name-input")
            yield Static("", id="error-label")
        yield Footer()

    def on_mount(self) -> None:
        self._render_step()

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _set_title(self, text: str) -> None:
        self.query_one("#step-title", Static).update(text)

    def _set_hint(self, text: str) -> None:
        self.query_one("#step-hint", Static).update(text)

    def _set_error(self, text: str) -> None:
        self.query_one("#error-label", Static).update(text)

    def _show_list_widgets(self) -> None:
        self.query_one("#filter-input").remove_class("hidden")
        self.query_one("#item-list").remove_class("hidden")
        self.query_one("#session-name-input").add_class("hidden")

    def _show_name_widgets(self) -> None:
        self.query_one("#filter-input").add_class("hidden")
        self.query_one("#item-list").add_class("hidden")
        self.query_one("#session-name-input").remove_class("hidden")

    # ------------------------------------------------------------------
    # Step rendering
    # ------------------------------------------------------------------

    @property
    def _total_steps(self) -> int:
        return 4 if self._subsystems else 3

    def _render_step(self) -> None:
        self._awaiting_repo_path = False
        self._set_error("")
        if self._step == 1:
            self._render_repo_step()
        elif self._step == 2:
            self._render_branch_step()
        elif self._step == 3:
            self._render_name_step()
        elif self._step == 4:
            self._render_subsystem_step()

    def _render_repo_step(self) -> None:
        self._set_title("Step 1 — Select Repository")
        self._set_hint("Type to filter. Enter to select. Escape to cancel.")
        self._show_list_widgets()

        fi = self.query_one("#filter-input", Input)
        fi.placeholder = "Filter repos…"
        fi.value = ""

        # Scan ~/dev/ for directories (excluding worktrees)
        self._dev_dirs: list[tuple[str, str]] = []  # (name, path)
        dev_dir = Path.home() / "dev"
        if dev_dir.is_dir():
            for entry in sorted(dev_dir.iterdir()):
                if entry.is_dir() and entry.name != "worktrees" and not entry.name.startswith("."):
                    self._dev_dirs.append((entry.name, str(entry)))

        self._populate_repo_list_from_dirs(self._dev_dirs)
        fi.focus()

    def _render_branch_step(self) -> None:
        assert self._selected_repo is not None
        self._set_title(f"Step 2 — Select Branch  [dim]({self._selected_repo.name})[/dim]")
        self._set_hint("Type to filter or enter a new branch name. Enter to confirm.")
        self._show_list_widgets()

        fi = self.query_one("#filter-input", Input)
        fi.placeholder = "Filter or type a new branch…"
        fi.value = ""

        try:
            self._branches = list_branches(self._selected_repo.path)
        except GitError as exc:
            self._branches = []
            self._set_error(str(exc))

        self._populate_branch_list(self._branches, "")
        fi.focus()

    def _render_name_step(self) -> None:
        assert self._selected_branch is not None
        self._set_title("Step 3 — Session Name")
        self._set_hint("Edit the name then press Enter to create. Escape to go back.")
        self._show_name_widgets()

        suggested = _sanitize_for_tmux(self._selected_branch)
        ni = self.query_one("#session-name-input", Input)
        ni.value = suggested
        ni.placeholder = "Session name…"
        ni.cursor_position = len(suggested)
        ni.focus()

    # ------------------------------------------------------------------
    # List population
    # ------------------------------------------------------------------

    def _populate_repo_list_from_dirs(self, dirs: list[tuple[str, str]]) -> None:
        self._render_seq += 1
        seq = self._render_seq
        self._id_to_dir: dict[str, tuple[str, str]] = {}
        lv = self.query_one("#item-list", ListView)
        lv.clear()
        for name, path in dirs:
            widget_id = f"dir-{_safe_id(name)}-{seq}"
            self._id_to_dir[widget_id] = (name, path)
            lv.append(ListItem(Label(f"[bold]{name}[/bold]  [dim]{path}[/dim]"), id=widget_id))
        lv.append(ListItem(Label("[green]+ Add new repo path…[/green]"), id=f"add-repo-{seq}"))

    def _populate_branch_list(self, branches: list[str], query: str) -> None:
        self._render_seq += 1
        seq = self._render_seq
        self._id_to_branch.clear()
        lv = self.query_one("#item-list", ListView)
        lv.clear()
        for branch in branches:
            widget_id = f"branch-{_safe_id(branch)}-{seq}"
            self._id_to_branch[widget_id] = branch
            lv.append(ListItem(Label(branch), id=widget_id))
        if query and query not in branches:
            lv.append(ListItem(Label(f"[green]+ New branch: [bold]{query}[/bold][/green]"), id=f"new-branch-{seq}"))

    # ------------------------------------------------------------------
    # Events
    # ------------------------------------------------------------------

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id != "filter-input" or self._awaiting_repo_path:
            return
        query = event.value.lower()
        if self._step == 1:
            filtered = [(n, p) for n, p in self._dev_dirs if query in n.lower() or query in p.lower()]
            self._populate_repo_list_from_dirs(filtered)
        elif self._step == 2:
            filtered = [b for b in self._branches if query in b.lower()]
            self._populate_branch_list(filtered, event.value)
        elif self._step == 4:
            filtered = [s for s in self._subsystems if query in s.lower()]
            self._populate_subsystem_list(filtered)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "filter-input":
            if self._awaiting_repo_path:
                self._finish_add_repo(event.value)
            else:
                self._confirm_list_selection(typed_value=event.value)
        elif event.input.id == "session-name-input":
            self._confirm_session_name(event.value)

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        item_id = event.item.id
        if self._step == 1:
            self._select_repo_item(item_id)
        elif self._step == 4:
            self._select_subsystem_item(item_id)
        elif self._step == 2:
            self._select_branch_item(item_id)

    # ------------------------------------------------------------------
    # Step logic
    # ------------------------------------------------------------------

    def _confirm_list_selection(self, typed_value: str) -> None:
        """Attempt to advance from a list step using the highlighted item or typed text."""
        lv = self.query_one("#item-list", ListView)
        highlighted = lv.highlighted_child
        # Default to first item if nothing is highlighted
        if highlighted is None and len(lv.children) > 0:
            highlighted = lv.children[0]
        if highlighted is not None:
            item_id = highlighted.id
            if self._step == 1:
                self._select_repo_item(item_id)
            elif self._step == 2:
                self._select_branch_item(item_id)
            elif self._step == 4:
                self._select_subsystem_item(item_id)
        elif self._step == 2 and typed_value:
            # Treat typed text as a new branch name (no list match needed)
            self._selected_branch = typed_value
            self._step = 3
            self._render_step()

    def _select_repo_item(self, item_id: str | None) -> None:
        if item_id and item_id.startswith("add-repo"):
            self._enter_repo_path_mode()
            return
        if item_id and item_id in self._id_to_dir:
            name, path = self._id_to_dir[item_id]

            class _DirRepo:
                def __init__(self, n: str, p: str) -> None:
                    self.path = p
                    self.name = n
                    self.id = None

            self._selected_repo = _DirRepo(name, path)  # type: ignore[assignment]
            self._step = 2
            self._render_step()

    def _select_branch_item(self, item_id: str | None) -> None:
        if item_id and item_id.startswith("new-branch"):
            typed = self.query_one("#filter-input", Input).value
            if typed:
                self._selected_branch = typed
                self._step = 3
                self._render_step()
            return
        if item_id and item_id in self._id_to_branch:
            self._selected_branch = self._id_to_branch[item_id]
            self._step = 3
            self._render_step()

    def _confirm_session_name(self, name: str) -> None:
        name = name.strip()
        if not name:
            self._set_error("Session name cannot be empty.")
            return
        existing = get_session_by_name(self._manager._conn, name)
        if existing is not None:
            self._set_error(f"Session '{name}' already exists. Choose a different name.")
            return
        self._session_name = name

        # Check for subsystems
        assert self._selected_repo is not None
        self._subsystems = detect_subsystems(self._selected_repo.path)
        if self._subsystems:
            self._step = 4
            self._render_step()
        else:
            self._create_session(name)

    def _render_subsystem_step(self) -> None:
        assert self._selected_repo is not None
        self._set_title(f"Step 4 — Working Directory  [dim]({self._selected_repo.name})[/dim]")
        self._set_hint("Pick a subsystem to start in, or select root. Enter to confirm.")
        self._show_list_widgets()

        fi = self.query_one("#filter-input", Input)
        fi.placeholder = "Filter…"
        fi.value = ""

        self._populate_subsystem_list(self._subsystems)
        fi.focus()

    def _populate_subsystem_list(self, subsystems: list[str]) -> None:
        self._render_seq += 1
        seq = self._render_seq
        self._id_to_subsystem.clear()
        lv = self.query_one("#item-list", ListView)
        lv.clear()
        # Root option first
        root_id = f"subsys-root-{seq}"
        self._id_to_subsystem[root_id] = ""
        lv.append(ListItem(Label("[bold]/ (root)[/bold]"), id=root_id))
        for sub in subsystems:
            widget_id = f"subsys-{_safe_id(sub)}-{seq}"
            self._id_to_subsystem[widget_id] = sub
            lv.append(ListItem(Label(sub), id=widget_id))

    def _select_subsystem_item(self, item_id: str | None) -> None:
        if item_id and item_id in self._id_to_subsystem:
            subdir = self._id_to_subsystem[item_id]
            self._selected_subdirectory = subdir or None
            self._create_session(self._session_name)

    def _create_session(self, session_name: str) -> None:
        assert self._selected_repo is not None
        assert self._selected_branch is not None
        try:
            self._manager.create_session(
                repo_path=self._selected_repo.path,
                base_branch=self._selected_branch,
                session_name=session_name,
                subdirectory=self._selected_subdirectory,
            )
        except Exception as exc:
            self._set_error(f"Error: {exc}")
            return

        try:
            tmux.switch_client(session_name)
        except tmux.TmuxError:
            pass
        self.app.exit()

    # ------------------------------------------------------------------
    # Add-repo-by-path sub-flow
    # ------------------------------------------------------------------

    def _enter_repo_path_mode(self) -> None:
        self._awaiting_repo_path = True
        self._set_hint("Enter the full path to a git repo, then press Enter.")
        fi = self.query_one("#filter-input", Input)
        fi.placeholder = "e.g. /home/you/dev/myproject…"
        fi.value = ""
        fi.focus()

    def _finish_add_repo(self, path_str: str) -> None:
        self._awaiting_repo_path = False
        path = Path(path_str.strip()).expanduser().resolve()
        if not path.is_dir() or not (path / ".git").exists():
            self._set_error(f"'{path}' is not a git repository.")
            fi = self.query_one("#filter-input", Input)
            fi.placeholder = "Filter repos…"
            fi.value = ""
            self._set_hint("Type to filter. Enter to select. Escape to cancel.")
            return
        # Build a lightweight stand-in; the DB repo will be created in create_session()
        # We need something with .path and .name for _render_branch_step
        class _AdhocRepo:
            def __init__(self, p: Path) -> None:
                self.path = str(p)
                self.name = p.name
                self.id = None

        self._selected_repo = _AdhocRepo(path)  # type: ignore[assignment]
        self._step = 2
        self._render_step()

    # ------------------------------------------------------------------
    # Navigation
    # ------------------------------------------------------------------

    def action_go_back(self) -> None:
        if self._awaiting_repo_path:
            # Cancel path entry, return to normal repo listing
            self._awaiting_repo_path = False
            fi = self.query_one("#filter-input", Input)
            fi.placeholder = "Filter repos…"
            fi.value = ""
            self._populate_repo_list_from_dirs(self._dev_dirs)
            self._set_hint("Type to filter. Enter to select. Escape to cancel.")
            self._set_error("")
            return
        if self._step == 1:
            self.app.pop_screen()
        else:
            self._step -= 1
            if self._step == 1:
                self._selected_repo = None
            elif self._step == 2:
                self._selected_branch = None
            elif self._step == 3:
                self._selected_subdirectory = None
            self._render_step()
