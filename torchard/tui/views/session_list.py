"""Main session list screen."""

from __future__ import annotations

import re
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Input

from torchard.core import tmux
from torchard.core.claude_session import classify_pane, get_session_id, get_first_user_message, summarize_message
from torchard.core.models import SessionInfo
from torchard.core.fuzzy import fuzzy_match
from torchard.core.manager import Manager
from torchard.tui.utils import truncate_end
from torchard.tui.switch import write_switch

# Consistent colors for repos - assigned by hash of repo name
_REPO_COLORS = [
    "#00aaff",  # blue
    "#ff6b6b",  # red
    "#51cf66",  # green
    "#ffd43b",  # yellow
    "#cc5de8",  # purple
    "#ff922b",  # orange
    "#22b8cf",  # cyan
    "#f06595",  # pink
]


def _repo_color(repo_name: str) -> str:
    return _REPO_COLORS[hash(repo_name) % len(_REPO_COLORS)]


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
        Binding("n", "new_picker", "New"),
        Binding("d", "delete", "Delete"),
        Binding("h", "history", "History"),
        Binding("full_stop", "action_menu", "Actions"),
        Binding("S", "settings", "Settings"),
        Binding("question_mark", "help", "Help"),
    ]

    def __init__(self, manager: Manager) -> None:
        super().__init__()
        self._manager = manager
        self._sessions: list[SessionInfo] = []
        self._repos: dict = {}
        self._expanded: set[str] = set()  # session row keys that are expanded
        self._filter: str = ""

    def compose(self) -> ComposeResult:
        yield Input(placeholder="Type to filter…", id="session-filter", classes="hidden")
        yield DataTable(id="session-table", cursor_type="row", zebra_stripes=False)
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one(DataTable)
        table.add_columns("Session", "Repo", "Branch")
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

    def _session_sort_key(self, s: SessionInfo, grouped: bool) -> tuple:
        """Composite sort key for sessions.

        Order: "main" pinned first, then sessions with recent activity (newest first),
        then sessions without activity (alphabetical). When *grouped*, adds repo name
        as a secondary key so sessions cluster by repo.
        """
        is_main = 0 if s.name == "main" else 1
        has_ts = 0 if s.last_selected_at else 1
        # Negate timestamp so newest sorts first (lexicographic descending).
        ts = s.last_selected_at or ""
        neg_ts = "".join(chr(0x10FFFF - ord(c)) for c in ts) if ts else "~"
        if grouped:
            repo = self._repos.get(s.repo_id) if s.repo_id else None
            repo_name = (repo.name if repo else "zzz").lower()
            return (is_main, repo_name, has_ts, neg_ts)
        return (is_main, has_ts, neg_ts)

    def _sorted_sessions(self) -> list[SessionInfo]:
        """Return sessions sorted and filtered for display."""
        sessions = self._manager.list_sessions()

        if self._filter:
            # Fuzzy filter: rank by match quality
            scored: list[tuple[SessionInfo, int]] = []
            for session in sessions:
                repo = self._repos.get(session.repo_id) if session.repo_id else None
                candidates = [session.name, repo.name if repo else "", session.base_branch or ""]
                best = None
                for c in candidates:
                    s = fuzzy_match(self._filter, c)
                    if s is not None and (best is None or s < best):
                        best = s
                if best is not None:
                    scored.append((session, best))
            scored.sort(key=lambda x: x[1])
            return [s for s, _ in scored]

        # No filter: sort with repo grouping
        sessions.sort(key=lambda s: self._session_sort_key(s, grouped=True))
        return sessions

    def _render_session_rows(self, table: DataTable, all_windows: dict[str, list[dict]]) -> None:
        """Populate the DataTable with rows for each session (and expanded child windows)."""
        last_repo_name = None
        for session in self._sessions:
            repo = self._repos.get(session.repo_id) if session.repo_id else None
            repo_name = repo.name if repo else ""
            branch = session.base_branch or "-"
            base = repo.default_branch if repo else "-"
            windows = str(session.windows) if session.windows is not None else "-"
            color = _repo_color(repo_name) if repo_name else "#666666"

            # Status indicator
            if session.attached:
                dot = "[green]●[/green]"
            elif not session.managed:
                dot = "[dim]◇[/dim]"
            elif session.live:
                dot = "[blue]○[/blue]"
            else:
                dot = " "

            row_key = str(session.id) if session.id is not None else f"unmanaged:{session.name}"
            expanded = row_key in self._expanded
            can_expand = session.live
            expand = "▾" if expanded else "▸" if can_expand else " "

            name_display = session.name
            if session.attached:
                name_display = f"[green]{name_display}[/green]"
            win_display = f" [dim]({windows})[/dim]" if windows != "-" else ""

            if branch == base or branch == "-":
                branch_display = branch
            else:
                branch_display = f"{base} → {branch}"

            # Show repo name colored, only on first session in a group (when not filtering)
            if not self._filter and repo_name != last_repo_name and repo_name:
                repo_display = f"[{color}]{truncate_end(repo_name, 20)}[/{color}]"
            elif repo_name:
                repo_display = f"[dim]{truncate_end(repo_name, 20)}[/dim]"
            else:
                repo_display = "[dim]-[/dim]"
            last_repo_name = repo_name

            table.add_row(
                f"{dot} {expand} {name_display}{win_display}",
                repo_display,
                branch_display,
                key=row_key,
            )

            if expanded and session.live:
                tmux_windows = all_windows.get(session.name, [])
                wt_by_path: dict[str, str] = {}
                if session.managed and session.id is not None:
                    for wt in self._manager.get_worktrees_for_session(session.id):
                        wt_by_path[wt.path] = wt.branch
                for i, win in enumerate(tmux_windows):
                    is_last = i == len(tmux_windows) - 1
                    prefix = "└" if is_last else "├"
                    wt_branch = wt_by_path.get(win["path"])
                    cmd = win.get("command", "")
                    is_claude = bool(cmd and re.match(r"^\d+\.\d+\.\d+", cmd))
                    if is_claude and re.match(r"^\d+\.\d+\.\d+", win["name"]):
                        _try_rename_claude_window(session.name, win)
                    if is_claude:
                        pane_text = tmux.capture_pane(f"{session.name}:{win['index']}", 8)
                        state = classify_pane(pane_text)
                        state_display = {
                            "thinking": "[#E87B35]✦ thinking…[/#E87B35]",
                            "working": "[#E87B35]✦ working…[/#E87B35]",
                            "prompting": "[#ff6b6b]✦ needs input[/#ff6b6b]",
                            "waiting": "[#E87B35]✦ waiting[/#E87B35]",
                            "idle": "[dim]✦ idle[/dim]",
                        }
                        cmd_display = state_display.get(state, "[#E87B35]✦ claude[/#E87B35]")
                    elif cmd and cmd != "zsh":
                        cmd_display = f"[italic]{cmd}[/italic]"
                    else:
                        cmd_display = ""
                    col_detail = f"[dim]wt:[/dim] {wt_branch}" if wt_branch else f"[dim]{truncate_end(win['path'], 30)}[/dim]"
                    table.add_row(
                        f"      [dim]{prefix}[/dim] [dim]{win['name']}[/dim]",
                        cmd_display,
                        col_detail,
                        key=f"win:{session.name}:{win['index']}",
                    )

    def _refresh_table(self, restore_key: str | None = None) -> None:
        self._repos = {r.id: r for r in self._manager.get_repos()}
        self._sessions = self._sorted_sessions()
        all_windows = tmux.list_all_windows()

        table = self.query_one(DataTable)
        table.clear()
        self._render_session_rows(table, all_windows)

        if self._sessions:
            target_row = 0
            if restore_key is not None:
                for i, rk in enumerate(table.rows):
                    if rk.value == restore_key:
                        target_row = i
                        break
            table.move_cursor(row=target_row)

    def _touch_by_name(self, session_name: str) -> None:
        """Update last_selected_at for a session looked up by name."""
        session = next((s for s in self._sessions if s.name == session_name and s.managed and s.id), None)
        if session:
            self._manager.touch_session(session.id)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        row_key = event.row_key.value
        if row_key and row_key.startswith("win:"):
            parts = row_key.split(":", 2)
            self._touch_by_name(parts[1])
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
            self._touch_by_name(session_name)
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
        if session is None or not session.live:
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
        if session.managed and session.id is not None:
            self._manager.touch_session(session.id)
        write_switch({"type": "session", "target": session.name})
        self.app.exit()

    def _session_for_row_key(self, row_key: str) -> SessionInfo | None:
        if row_key.startswith("unmanaged:"):
            name = row_key[len("unmanaged:"):]
            return next((s for s in self._sessions if s.name == name), None)
        session_id = int(row_key)
        return next((s for s in self._sessions if s.id == session_id), None)

    def action_quit(self) -> None:
        self.app.exit()

    def _current_session(self) -> SessionInfo | None:
        """Get the session for the current row, or None if on a child row."""
        row_key = self._current_row_key()
        if row_key is None or self._is_child_row(row_key):
            return None
        return self._session_for_row_key(row_key)

    # ------------------------------------------------------------------
    # New… picker (n)
    # ------------------------------------------------------------------

    def action_new_picker(self) -> None:
        from torchard.tui.views.action_menu import ActionMenu
        session = self._current_session()
        items: list[tuple[str, str, str]] = [
            ("new-session", "New session", ""),
        ]
        if session and session.managed:
            items.append(("new-tab", f"New tab in {session.name}", ""))
        repo = self._repos.get(session.repo_id) if session and session.repo_id else None
        if repo:
            items.append(("review", "Review PR/branch", repo.name))
        self.app.push_screen(ActionMenu("New…", items), self._on_new_picked)

    def _on_new_picked(self, key: str | None) -> None:
        if key is None:
            return
        session = self._current_session()
        if key == "new-session":
            from torchard.tui.views.new_session import NewSessionScreen
            self.app.push_screen(NewSessionScreen(self._manager))
        elif key == "new-tab" and session and session.managed:
            from torchard.tui.views.new_tab import NewTabScreen
            self.app.push_screen(NewTabScreen(self._manager, session.id, session.name))
        elif key == "review" and session and session.repo_id:
            from torchard.tui.views.review import ReviewScreen
            repo = self._repos.get(session.repo_id)
            if repo:
                self.app.push_screen(ReviewScreen(self._manager, repo.path, repo.name))

    # ------------------------------------------------------------------
    # Context-aware delete (d)
    # ------------------------------------------------------------------

    def action_delete(self) -> None:
        from torchard.tui.views.confirm import ConfirmModal
        row_key = self._current_row_key()
        if row_key is None:
            return
        # Kill a tab
        if row_key.startswith("win:"):
            parts = row_key.split(":", 2)
            session_name = parts[1]
            window_index = int(parts[2])

            def on_confirm_tab(confirmed: bool) -> None:
                if not confirmed:
                    return
                try:
                    tmux.kill_window(session_name, window_index)
                except tmux.TmuxError:
                    pass
                self._refresh_table()

            self.app.push_screen(
                ConfirmModal(f"Kill tab {window_index} in '{session_name}'?", "This will close the window and any processes in it."),
                on_confirm_tab,
            )
            return
        # Delete a session
        session = self._session_for_row_key(row_key)
        if session is None:
            return
        if session.managed:
            name = session.name
            msg = "Remove from torchard."
            if session.live:
                msg += " The tmux session will also be killed."

            def on_confirm(confirmed: bool) -> None:
                if not confirmed:
                    return
                self._manager.delete_session(session.id, cleanup_worktrees=False)
                self._refresh_table()

            self.app.push_screen(
                ConfirmModal(f"Delete session '{name}'?", msg),
                on_confirm,
            )
        else:
            name = session.name

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

    # ------------------------------------------------------------------
    # Action menu (.)
    # ------------------------------------------------------------------

    def action_action_menu(self) -> None:
        from torchard.tui.views.action_menu import ActionMenu
        row_key = self._current_row_key()
        if row_key is None:
            return

        # Tab-level actions
        if row_key.startswith("win:"):
            parts = row_key.split(":", 2)
            session_name = parts[1]
            window_index = int(parts[2])
            windows = tmux.list_windows(session_name)
            win = next((w for w in windows if w["index"] == window_index), None)
            items: list[tuple[str, str, str]] = []
            if win:
                items.append(("rename-tab", "Rename tab", win["name"]))
            self.app.push_screen(ActionMenu("Tab actions", items), self._on_action_picked)
            return

        session = self._current_session()
        if session is None:
            return

        items = []
        if session.managed:
            items.append(("rename", "Rename", session.name))
            items.append(("branch", "Change branch", session.base_branch or ""))
            if session.live:
                items.append(("claude", "Launch claude", ""))
        elif session.live:
            items.append(("adopt", "Adopt session", "bring under torchard management"))
        items.append(("cleanup", "Cleanup worktrees", ""))

        self.app.push_screen(ActionMenu(f"Actions — {session.name}", items), self._on_action_picked)

    def _on_action_picked(self, key: str | None) -> None:
        if key is None:
            return
        row_key = self._current_row_key()
        session = self._current_session()

        if key == "rename-tab" and row_key and row_key.startswith("win:"):
            from torchard.tui.views.rename_session import RenameWindowScreen
            parts = row_key.split(":", 2)
            session_name = parts[1]
            window_index = int(parts[2])
            windows = tmux.list_windows(session_name)
            win = next((w for w in windows if w["index"] == window_index), None)
            if win:
                self.app.push_screen(RenameWindowScreen(session_name, window_index, win["name"]))
            return

        if session is None:
            return
        if key == "rename" and session.managed:
            from torchard.tui.views.rename_session import RenameSessionScreen
            self.app.push_screen(RenameSessionScreen(self._manager, session.id, session.name))
        elif key == "branch" and session.managed:
            from torchard.tui.views.edit_branch import EditBranchScreen
            self.app.push_screen(EditBranchScreen(self._manager, session.id, session.name))
        elif key == "claude" and session.live:
            tmux.new_window(session.name, "claude")
            tmux.send_keys(f"{session.name}:claude", "claude", "Enter")
            if session.managed and session.id is not None:
                self._manager.touch_session(session.id)
            write_switch({"type": "session", "target": session.name})
            self.app.exit()
        elif key == "adopt" and not session.managed:
            from torchard.tui.views.adopt_session import AdoptSessionScreen
            self.app.push_screen(AdoptSessionScreen(self._manager, session.name))
        elif key == "cleanup":
            from torchard.tui.views.cleanup import CleanupScreen
            self.app.push_screen(CleanupScreen(self._manager))

    def action_history(self) -> None:
        session = self._current_session()
        scope_paths = None
        scope_label = None
        if session and session.managed:
            repo = self._repos.get(session.repo_id)
            if repo:
                # Scope to repo path + all its worktree paths
                paths = [repo.path]
                if session.id is not None:
                    for wt in self._manager.get_worktrees_for_session(session.id):
                        paths.append(wt.path)
                # Also include worktrees root for this repo
                wt_root = str(self._manager.worktrees_dir / repo.name)
                paths.append(wt_root)
                scope_paths = paths
                scope_label = session.name
        from torchard.tui.views.history import HistoryScreen
        self.app.push_screen(HistoryScreen(self._manager, scope_paths, scope_label))

    def action_settings(self) -> None:
        from torchard.tui.views.settings import SettingsScreen
        self.app.push_screen(SettingsScreen(self._manager))

    def action_help(self) -> None:
        from torchard.tui.views.help import HelpScreen
        self.app.push_screen(HelpScreen())



def _try_rename_claude_window(session_name: str, win: dict) -> None:
    """Rename a claude window from its version number to the first user message."""
    session_id = get_session_id(win.get("pane_pid", ""))
    if not session_id:
        return
    msg = get_first_user_message(session_id)
    if not msg:
        return
    name = summarize_message(msg)
    try:
        tmux.rename_window(session_name, win["index"], name)
        win["name"] = name
    except tmux.TmuxError:
        pass
