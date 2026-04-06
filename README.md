# torchard

tmux session and git worktree manager. Replaces the grab-bag of shell scripts I was using to manage parallel development across repos and feature branches.

The core idea: each tmux session is bound to a repo and branch. New tabs within a session automatically create worktrees. A TUI (built with [textual](https://github.com/Textualize/textual)) handles creation, navigation and cleanup - replacing the old fzf session picker, worktree picker and various one-off scripts.

## Install

Requires Python 3.11+ and [uv](https://docs.astral.sh/uv/).

```
cd ~/dev/torchard
uv sync
```

Wire it into tmux as a popup (replaces the default session picker):

```tmux
bind -n M-s display-popup -E -w 80% -h 70% "/path/to/torchard/.venv/bin/torchard"
```

On first launch, torchard scans `~/dev/` for repos, `~/dev/worktrees/` for existing worktrees and matches live tmux sessions to repos.

## What it does

**Session list** is the main view. Shows all tmux sessions (managed and unmanaged) sorted with the current session on top. `enter` to switch, `tab` to expand and see live tmux windows with running commands. Claude sessions show up with a `✦` marker.

**New session** (`n`) walks you through picking a repo from `~/dev/`, a branch, a name and (for monorepos) a working subdirectory. Feature branches get a worktree at `~/dev/worktrees/<repo>/<branch>` with a 3-window layout: claude, diff and shell.

**New tab** (`w`) creates a worktree branching from the session's branch and launches claude in it.

**PR checkout** (`p`) takes a PR number or branch name, fetches it, creates a worktree and session, launches claude, switches you there.

**History** (`h`) browses your Claude conversation index (`~/.claude/conversation-index.md`). Scoped to the current session's repo or global. `enter` resumes a conversation with `claude --resume` in the right directory.

**Cleanup** (`c`) shows all worktrees with async staleness detection (merged or remote-deleted branches). Select and bulk-delete.

## Keybinds

| Key | Action |
|-----|--------|
| `enter` | Switch to session (or specific window if expanded) |
| `tab` | Expand/collapse session windows |
| `/` | Filter sessions |
| `n` | New session |
| `w` | New worktree tab |
| `g` | Launch claude in session |
| `p` | Checkout PR/branch |
| `d` | Delete session |
| `r` | Rename session or tab |
| `b` | Change session branch |
| `a` | Adopt unmanaged session |
| `h` | Conversation history |
| `c` | Cleanup worktrees |
| `x` | Kill tab (on expanded window) |
| `?` | Help |
| `q` | Quit |

## Data

Session and worktree metadata lives in `~/.local/share/torchard/torchard.db` (SQLite). The DB is the source of truth for managed sessions - torchard doesn't use the older `wt-registry` system.

Worktrees follow the convention `~/dev/worktrees/<repo-name>/<branch-name>`.
