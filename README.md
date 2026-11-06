# trellis

TUI for managing tmux sessions and git worktrees together.

Each tmux session is bound to a repo and branch. New tabs within a session automatically create worktrees. trellis handles session creation, navigation, worktree lifecycle and cleanup from a single interface.

## Install

Requires a Rust toolchain ([rustup](https://rustup.rs/)).

```
git clone git@github.com:tomplex/trellis.git
cd trellis
cargo install --path .
```

This installs the `trellis` binary into `~/.cargo/bin`. Add a tmux keybind to launch it as a popup:

```tmux
bind -n M-s display-popup -E -w 80% -h 70% "trellis"
```

On first launch, trellis opens its settings screen so you can confirm where your repos and worktrees live (defaults: `~/dev` and `~/dev/worktrees`). Once you close settings, trellis scans those directories and discovers live tmux sessions.

## Agent

Trellis is agent-agnostic. The `agent_cmd` setting (default `pi`) names the coding agent launched in each session's `agent` window; `omp` and `claude` work too. Whatever it is must accept `-c` (continue the most recent conversation for the cwd, used to revive dead sessions) and `-r` (interactive resume picker, used by `H`).

Live status (`working…` / `needs input` / `idle`) and the window auto-rename come from a small status file the agent writes, not from scraping the pane. For pi, install the bundled extension:

```
ln -s ~/projects/trellis/pi/trellis.ts ~/.pi/agent/extensions/trellis.ts
```

`needs input` is driven by pi's `ui_prompt_start`/`ui_prompt_end` events, so any extension that blocks on a `ctx.ui` prompt (an approval gate, `ask_question`, …) is reported without knowing about trellis. Pi has no permission prompts by default; [`@czottmann/pi-automode`](https://github.com/czottmann/pi-automode) gives it a Claude Code-style auto mode and prompts through `ctx.ui` for `permissions.ask` rules.

Any agent can participate by writing `~/.local/share/trellis/panes/<TMUX_PANE>.json` (leading `%` dropped) and deleting it on exit:

```json
{ "agent": "pi", "pid": 12345, "state": "working", "title": "fix the login bug" }
```

`state` is `idle`, `working`, or `prompting`; `title` is optional and becomes the window name. Trellis ignores a file whose `pid` is no longer running.

## What it does

**Session list** is the main view. Shows all tmux sessions sorted with the current session on top. `enter` to switch, `tab` to expand and see live windows with running commands. Agent panes show up with a `✦` marker and a status (`working…`, `needs input`, `idle`). `/` to filter.

**New session** (`n`) walks through picking a repo, branch, session name and (for monorepos) a working subdirectory. Feature branches get a worktree with a 2-window layout: agent and shell.

**New tab** (`t`) creates a worktree branching from the session's branch and launches the agent in it.

**PR review** (`R`) takes a PR number or branch name (via `gh`), fetches it, creates a worktree and session, launches the agent, and switches to it.

**History** (`H`) opens the agent's own resume picker (`<agent> -r`) in a new window of the selected live session, scoped to that session's directory.

**Cleanup** (`c`) shows all worktrees with staleness detection (merged or remote-deleted branches). Select and bulk-delete.

**Actions menu** (`.`) opens a per-session menu — change branch, launch the agent, or adopt an unmanaged session.

**Settings** (`S`) configures the repos directories (one per line), the worktrees directory, and the agent command.

## Keybinds

| Key | Action |
|-----|--------|
| `enter` | Switch to session (or specific window if expanded) |
| `tab` | Expand/collapse session windows |
| `j` / `k` | Navigate up/down |
| `/` | Filter sessions |
| `n` | New session |
| `t` | New worktree tab |
| `T` | Plain tab (tmux window in the current directory, no worktree) |
| `R` | PR review (PR number or branch) |
| `r` | Rename session |
| `d` | Delete session or kill tab |
| `.` | Actions menu (change branch, launch claude, adopt) |
| `H` | Conversation history |
| `c` | Cleanup worktrees |
| `A` | Adopt an existing worktree as a new session |
| `S` | Settings |
| `?` | Help |
| `q` | Quit |

## Data

Session and worktree metadata lives in `~/.local/share/trellis/trellis.db` (SQLite); agent status files live beside it in `panes/`. Worktrees are created at `<worktrees_dir>/<repo-name>/<branch-name>` (default `~/dev/worktrees/`). Both directories are configurable in settings.
