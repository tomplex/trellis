"""Cleanup view for managing stale worktrees."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static

from torchard.core.db import get_sessions, get_worktrees
from torchard.core.manager import Manager
from torchard.core.models import Session, Worktree
from torchard.tui.utils import truncate_start
from torchard.tui.views.confirm import ConfirmModal


class CleanupScreen(Screen):
    """Cleanup view: shows all worktrees with stale indicators and checkboxes."""

    BINDINGS = [
        Binding("escape", "dismiss", "Back"),
        Binding("space,enter", "toggle_selected", "Toggle"),
        Binding("a", "select_all", "Select all"),
        Binding("A", "deselect_all", "Deselect all", show=False),
        Binding("d", "delete_selected", "Delete selected"),
        Binding("j,down", "cursor_down", "Down", show=False),
        Binding("k,up", "cursor_up", "Up", show=False),
    ]

    def __init__(self, manager: Manager) -> None:
        super().__init__()
        self._manager = manager
        # row_key -> worktree
        self._worktrees: dict[str, Worktree] = {}
        # worktree id -> session name (or "Unattached")
        self._session_names: dict[int, str] = {}
        # set of stale worktree ids
        self._stale_ids: set[int] = set()
        # set of selected worktree ids (by row key string)
        self._selected: set[str] = set()

    def compose(self) -> ComposeResult:
        yield Static("[bold]Cleanup Worktrees[/bold]", id="cleanup-title")
        yield Static("", id="cleanup-status")
        yield DataTable(id="cleanup-table", cursor_type="row", zebra_stripes=False)
        yield Footer()

    def on_mount(self) -> None:
        # Build session map
        sessions: list[Session] = get_sessions(self._manager._conn)
        session_by_id: dict[int, Session] = {s.id: s for s in sessions if s.id is not None}

        # Load worktrees
        all_worktrees: list[Worktree] = get_worktrees(self._manager._conn)

        # Build session name lookup keyed by worktree session_id
        for wt in all_worktrees:
            if wt.id is None:
                continue
            if wt.session_id is not None and wt.session_id in session_by_id:
                self._session_names[wt.id] = session_by_id[wt.session_id].name
            else:
                self._session_names[wt.id] = "Unattached"
            self._worktrees[str(wt.id)] = wt

        # Build table immediately (staleness checked in background)
        table = self.query_one(DataTable)
        table.add_columns("", "Branch", "Session", "Path", "Status")

        sorted_worktrees = sorted(
            all_worktrees,
            key=lambda w: (
                self._session_names.get(w.id, "Unattached") if w.id is not None else "Unattached",
                w.branch,
            ),
        )

        for wt in sorted_worktrees:
            if wt.id is None:
                continue
            key = str(wt.id)
            session_label = self._session_names.get(wt.id, "Unattached")
            table.add_row(
                "\\[ ]",
                wt.branch,
                session_label,
                truncate_start(wt.path, 50),
                "[dim]checking…[/dim]",
                key=key,
            )

        if all_worktrees:
            table.move_cursor(row=0)

        self._refresh_status()

        # Check staleness in background
        self.run_worker(self._check_staleness, thread=True)

    def _check_staleness(self) -> None:
        stale = self._manager.get_stale_worktrees()
        self._stale_ids = {wt.id for wt in stale if wt.id is not None}
        self.call_from_thread(self._apply_stale_status)

    def _apply_stale_status(self) -> None:
        table = self.query_one(DataTable)
        status_col_key = table.ordered_columns[4].key
        branch_col_key = table.ordered_columns[1].key
        for row_key, wt in self._worktrees.items():
            is_stale = wt.id in self._stale_ids
            status = _make_status(wt, is_stale)
            table.update_cell(row_key, status_col_key, status)
            if is_stale:
                table.update_cell(row_key, branch_col_key, f"[yellow]{wt.branch}[/yellow]")
        self._refresh_status()

    def _refresh_status(self) -> None:
        count = len(self._selected)
        total = len(self._worktrees)
        stale_count = len(self._stale_ids)
        status = self.query_one("#cleanup-status", Static)
        parts = [f"[dim]{total} worktree{'s' if total != 1 else ''}[/dim]"]
        if stale_count:
            parts.append(f"[yellow]{stale_count} stale[/yellow]")
        if count:
            parts.append(f"[bold #00aaff]{count} selected[/bold #00aaff]")
        status.update("  ".join(parts))

    def _update_row_checkbox(self, row_key: str) -> None:
        table = self.query_one(DataTable)
        checked = row_key in self._selected
        marker = "[bold #00aaff]\\[x][/bold #00aaff]" if checked else "\\[ ]"
        first_col_key = table.ordered_columns[0].key
        table.update_cell(row_key, first_col_key, marker)

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()

    def action_toggle_selected(self) -> None:
        table = self.query_one(DataTable)
        if table.row_count == 0:
            return
        row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        if row_key is None:
            return
        if row_key in self._selected:
            self._selected.discard(row_key)
        else:
            self._selected.add(row_key)
        self._update_row_checkbox(row_key)
        self._refresh_status()

    def action_select_all(self) -> None:
        for row_key in self._worktrees:
            self._selected.add(row_key)
            self._update_row_checkbox(row_key)
        self._refresh_status()

    def action_deselect_all(self) -> None:
        for row_key in list(self._selected):
            self._selected.discard(row_key)
            self._update_row_checkbox(row_key)
        self._refresh_status()

    def action_delete_selected(self) -> None:
        if not self._selected:
            return

        def on_confirm(confirmed: bool) -> None:
            if not confirmed:
                return
            errors: list[str] = []
            deleted_keys: list[str] = []
            for key in list(self._selected):
                wt = self._worktrees.get(key)
                if wt is None or wt.id is None:
                    continue
                try:
                    self._manager.cleanup_worktree(wt.id)
                    deleted_keys.append(key)
                except Exception as exc:
                    errors.append(f"{wt.branch}: {exc}")

            table = self.query_one(DataTable)
            for key in deleted_keys:
                self._selected.discard(key)
                self._stale_ids.discard(self._worktrees[key].id)
                del self._worktrees[key]
                table.remove_row(key)

            self._refresh_status()
            if errors:
                self.notify("\n".join(errors), severity="error", title="Errors during cleanup")

        count = len(self._selected)
        self.app.push_screen(
            ConfirmModal(
                f"Delete {count} worktree{'s' if count != 1 else ''}?",
                "This will remove the git worktree(s) and DB records.",
            ),
            on_confirm,
        )

    def action_dismiss(self) -> None:
        self.app.pop_screen()

    DEFAULT_CSS = """
    CleanupScreen {
        background: #1a1a2e;
    }
    #cleanup-title {
        color: #00aaff;
        text-style: bold;
        padding: 1 2 0 2;
    }
    #cleanup-status {
        color: #aaaaaa;
        padding: 0 2 1 2;
        height: 1;
    }
    DataTable {
        background: #1a1a2e;
        color: #e0e0e0;
        height: 1fr;
    }
    DataTable > .datatable--header {
        background: #16213e;
        color: #00aaff;
        text-style: bold;
    }
    DataTable > .datatable--cursor {
        background: #0f3460;
        color: #ffffff;
    }
    DataTable > .datatable--hover {
        background: #16213e;
    }
    Footer {
        background: #16213e;
        color: #aaaaaa;
    }
    Footer > .footer--highlight {
        background: #0f3460;
        color: #00aaff;
    }
    Footer > .footer--key {
        color: #00aaff;
        text-style: bold;
    }
    """


def _make_status(wt: Worktree, is_stale: bool) -> str:
    if not is_stale:
        return "[green]ok[/green]"
    # Stale — we don't have the specific reason here without re-running git,
    # so just show "stale" with a warning color
    return "[yellow]stale[/yellow]"


