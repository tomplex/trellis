# torchard implementation plan

Based on: `docs/superpowers/specs/2026-04-06-torchard-design.md`

## Task 1: Project scaffolding and data model

Set up the Python project with uv, create the SQLite database layer and domain models.

**Files to create:**
- `pyproject.toml` - Python project config with uv, textual dependency, `torchard` script entry point
- `torchard/__init__.py` - empty
- `torchard/__main__.py` - entry point that calls the TUI app
- `torchard/core/__init__.py` - empty
- `torchard/core/models.py` - dataclasses: `Repo`, `Session`, `Worktree`
- `torchard/core/db.py` - SQLite setup, table creation, CRUD operations for all three models

**Requirements:**
- `pyproject.toml` uses `[project.scripts]` to define `torchard = "torchard.__main__:main"`
- DB path: `~/.local/share/torchard/torchard.db` (create parent dirs if needed)
- Models are plain dataclasses with `id: int | None` for new vs persisted
- `db.py` provides: `init_db()`, `add_repo()`, `get_repos()`, `add_session()`, `get_sessions()`, `get_session_by_name()`, `add_worktree()`, `get_worktrees()`, `get_worktrees_for_session()`, `delete_session()`, `delete_worktree()`
- All timestamps are ISO 8601 strings
- Foreign keys enforced (`PRAGMA foreign_keys = ON`)
- `worktrees.session_id` is nullable
- Write tests for all CRUD operations using an in-memory SQLite database

**Acceptance criteria:**
- `uv run python -m pytest` passes with all DB tests green
- Models and DB layer are importable and functional

## Task 2: tmux and git CLI wrappers

Create thin wrappers around tmux and git CLI commands that the core manager will use.

**Files to create:**
- `torchard/core/tmux.py` - tmux CLI wrapper
- `torchard/core/git.py` - git/worktree CLI wrapper

**Requirements:**

`tmux.py` provides:
- `list_sessions() -> list[dict]` - returns session name, window count, attached status
- `new_session(name: str, start_dir: str) -> None`
- `switch_client(session_name: str) -> None`
- `new_window(session_name: str, window_name: str, start_dir: str) -> None`
- `kill_session(session_name: str) -> None`
- `session_exists(name: str) -> bool`
- All functions shell out via `subprocess.run` with appropriate error handling

`git.py` provides:
- `detect_default_branch(repo_path: str) -> str` - returns "main" or "master" by inspecting remote HEAD or local branches
- `list_branches(repo_path: str) -> list[str]` - local branches
- `list_worktrees(repo_path: str) -> list[dict]` - path, branch, commit for each worktree
- `create_worktree(repo_path: str, worktree_path: str, branch: str, base_branch: str) -> None` - creates branch from base_branch and checks out into worktree_path
- `remove_worktree(repo_path: str, worktree_path: str) -> None`
- `is_branch_merged(repo_path: str, branch: str, into: str) -> bool`
- `has_remote_branch(repo_path: str, branch: str) -> bool`

**Acceptance criteria:**
- Functions have correct subprocess calls
- Write tests that mock subprocess.run to verify correct command construction
- Error cases raise clear exceptions (e.g. session already exists, worktree path conflict)

## Task 3: Manager orchestration layer

Create the manager that ties DB, tmux, and git together into high-level operations.

**Files to create:**
- `torchard/core/manager.py`

**Dependencies:** Tasks 1 and 2

**Requirements:**

`Manager` class takes a db connection (or db path) and provides:
- `create_session(repo_path: str, base_branch: str, session_name: str) -> Session` - registers repo if needed (auto-detecting default branch), creates DB session, creates tmux session in repo directory, returns Session
- `add_tab(session_id: int, branch_name: str) -> Worktree` - looks up session's repo and base branch, creates worktree at `~/dev/worktrees/<repo_name>/<branch_name>`, creates git worktree branching from session's base branch, creates tmux window in the session pointing at worktree dir, records worktree in DB
- `delete_session(session_id: int, cleanup_worktrees: bool = False)` - kills tmux session, optionally removes all associated worktrees (git worktree remove + DB delete), removes DB session
- `cleanup_worktree(worktree_id: int)` - removes git worktree, deletes DB record
- `get_stale_worktrees() -> list[Worktree]` - worktrees whose branch is merged into the repo's default branch, or whose remote branch is deleted
- `scan_existing()` - first-run adoption: scans `~/dev/worktrees/` and `~/dev/` for repos, scans active tmux sessions, populates DB with discovered state. Best-effort matching of tmux sessions to repos based on working directory.
- `list_sessions() -> list[dict]` - combines DB session data with live tmux state (attached, window count)

**Acceptance criteria:**
- Write tests that mock the tmux and git wrappers to verify orchestration logic
- Correct worktree path construction: `~/dev/worktrees/<repo_name>/<branch_name>`
- Session creation registers repo if not already known
- Cleanup properly cascades (worktree removal in both git and DB)

## Task 4: TUI - main session list view

Build the textual app with the main session list view that supports navigation and session switching.

**Files to create:**
- `torchard/tui/__init__.py` - empty
- `torchard/tui/app.py` - textual App subclass
- `torchard/tui/views/__init__.py` - empty
- `torchard/tui/views/session_list.py` - main view

**Dependencies:** Task 3

**Requirements:**
- App launches showing the session list from `Manager.list_sessions()`
- Each row shows: session name, repo name (truncated), base branch (truncated)
- Highlighted row has distinct styling
- Keyboard navigation: up/down/j/k to move, enter to switch session (calls `tmux switch-client` and exits the app)
- Mouse: click a row to highlight, double-click or click+enter to switch
- Footer shows keybind hints: `[n] new  [w] tab  [d] delete  [c] cleanup  [?] help  [q] quit`
- `q` or `escape` exits without switching
- Keybinds `n`, `w`, `d`, `c` are bound but push placeholder screens for now (Tasks 5-7)
- App styling: minimal, dark background, blue accent (matching tmux config's colour39)

**Acceptance criteria:**
- `uv run torchard` launches the TUI and shows sessions
- Navigation and switching work
- App exits cleanly after switching or quitting

## Task 5: TUI - new session wizard

Implement the new session creation flow as a multi-step screen.

**Files to create:**
- `torchard/tui/views/new_session.py`

**Dependencies:** Task 4

**Requirements:**
- Step 1: Show list of known repos + option to "add new repo" (enter a path). Filterable by typing.
- Step 2: Show branches for selected repo, filterable. User can also type a new branch name.
- Step 3: Session name input, pre-populated from branch name (sanitized for tmux: no dots or colons). Editable.
- Enter confirms each step, escape goes back one step (or exits wizard from step 1)
- On completion: calls `Manager.create_session()`, switches to new session, exits app
- If session name already exists, show inline error and let user rename

**Acceptance criteria:**
- Full flow works: pick repo -> pick branch -> name session -> session created and switched to
- Escape navigation works at each step
- Duplicate session name is handled gracefully

## Task 6: TUI - new tab flow

Implement adding a new worktree tab to an existing session.

**Files to create:**
- `torchard/tui/views/new_tab.py`

**Dependencies:** Task 4

**Requirements:**
- Triggered from session list with `w` key (operates on highlighted session)
- Single input: branch/worktree name
- On confirm: calls `Manager.add_tab()`, exits app (the new window is created in the target session)
- If branch name already exists or worktree path conflicts, show inline error
- Escape cancels and returns to session list

**Acceptance criteria:**
- Creating a tab creates the worktree and tmux window in the correct session
- Error cases handled with user-visible messages

## Task 7: TUI - cleanup view

Implement the cleanup view for managing stale worktrees.

**Files to create:**
- `torchard/tui/views/cleanup.py`

**Dependencies:** Task 4

**Requirements:**
- Shows all worktrees grouped by session (or "unattached" for orphan worktrees)
- Each worktree shows: branch name, path, status indicators (merged, remote deleted, no session)
- Checkboxes for selection, select-all toggle
- "Delete selected" action with confirmation
- Calls `Manager.cleanup_worktree()` for each selected
- Escape returns to session list
- Stale worktrees (merged or remote-deleted) are visually highlighted

**Acceptance criteria:**
- Worktrees display with correct status indicators
- Selection and bulk deletion work
- Confirmation prevents accidental deletion
