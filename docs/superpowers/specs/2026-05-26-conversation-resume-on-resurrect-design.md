# Conversation Resume on Session Resurrect

When a dead trellis session is resurrected, launch its default claude window with `claude --resume <id>` instead of plain `claude`, where `<id>` is the most-recent conversation that ran in that worktree.

## Motivation

`resurrect_session` in `src/manager.rs` calls `apply_layout`, which sends a literal `claude\n` into the first window of every revived session. The prior conversation — even though Claude itself still has the full transcript on disk under `~/.claude/projects/` — is silently abandoned. Operators have to manually `claude --resume <uuid>` after the fact, after looking up the UUID, or just start over.

Trellis already understands `claude --resume` (used by the History view at `src/tui/history.rs:251`). The missing piece is wiring resurrect to find the right ID.

## Source of truth: `~/.claude/projects/`

Trellis's existing code touches three Claude-related paths:

| Path | Purpose | Writer present? |
|---|---|---|
| `~/.claude/conversation-index.md` | Markdown index parsed by `src/conversation_index.rs`; powers the History view | No writer ships with trellis; not present on the operator's machine |
| `/tmp/claude-sessions/pid-{pid}` | Pane-PID → conversation-ID map read by `src/claude_pane.rs::get_session_id` | No writer ships with trellis; not present on the operator's machine |
| `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl` | Claude Code's own conversation store | **Claude is the writer** — always populated |

Only the third path is actually populated on a standard Claude Code install. It is also the canonical source — Claude maintains it directly. This design uses it.

The first two paths are out of scope. Their absence on the operator's machine is a separate concern (a History view that silently shows empty results is a real bug, but solving it requires installing a hook that writes one of those files; that work is independent of this change).

## Approach

At resurrect time, after `wt_path` is resolved (existing logic at `src/manager.rs:211-228`):

1. Compute the encoded directory name from `wt_path`: replace `/` with `-` so `/Users/brendan/dev/worktrees/trellis/feat-x` becomes `-Users-brendan-dev-worktrees-trellis-feat-x`.
2. `read_dir` `~/.claude/projects/<encoded>/` and filter to entries with extension `jsonl`.
3. Return the file with the largest `metadata.modified()` — its file stem is the conversation UUID.
4. If the directory is absent, has no `.jsonl` files, or any I/O error occurs, return `None`.

If a UUID is found, send `claude --resume <uuid>` into the first window. Otherwise send plain `claude` (current behavior).

### Selection rule

Most recent by mtime. Ties broken arbitrarily (the first `.jsonl` encountered by `read_dir` order). Ties between two `.jsonl` files in the same worktree are vanishingly rare, and either answer is acceptable when they occur.

### Fallback ladder

1. Encoded dir exists with at least one `.jsonl` → resume the most-recent.
2. Encoded dir missing, or present but empty of `.jsonl` → plain `claude`.
3. I/O error reading the dir → plain `claude` (defensive; no log, no surface).

No user-visible signal on fallback. A tmux pane just shows `claude` starting fresh, identical to today.

### Scope

Only the first window of `DEFAULT_LAYOUT` (the `"claude"` window). The second window (`"shell"`) is unaffected. New-session creation (`find_or_create_session` → `apply_layout`) keeps sending plain `claude`. Ad-hoc claude tabs spawned via the actions menu are one-off side analyses and intentionally not resumed.

## Implementation

### New function in `src/claude_pane.rs`

```rust
pub fn find_latest_conversation(cwd: &Path) -> Option<String>
```

Returns the bare UUID (no `.jsonl` extension) of the newest conversation in `~/.claude/projects/<encoded-cwd>/`, or `None`.

Internally, split into a public entry point and a private helper that takes the projects directory as a parameter, mirroring the `parse_index(Some(path))` pattern in `src/conversation_index.rs`:

```rust
fn find_latest_conversation_in(projects_dir: &Path, cwd: &Path) -> Option<String>
```

The public wrapper resolves `projects_dir` via `dirs::home_dir()`. Tests call the inner function against a tempdir.

Encoding can be inlined: `cwd.to_string_lossy().replace('/', '-')`. No leading-slash adjustment needed — the existing dir-name convention in `~/.claude/projects/` matches this transform exactly.

### Refactor `apply_layout` in `src/manager.rs`

Parameterize the first window's claude command. Today:

```rust
const DEFAULT_LAYOUT: &[(&str, Option<&str>)] = &[
    ("claude", Some("claude")),
    ("shell", None),
];

pub fn apply_layout(session_name: &str, working_dir: &str)
```

After:

```rust
pub fn apply_layout(session_name: &str, working_dir: &str, claude_cmd: &str)
```

Inside, the first window's send-keys uses `claude_cmd` instead of `DEFAULT_LAYOUT[0].1`. Set `DEFAULT_LAYOUT[0].1 = None` — `apply_layout` no longer owns the claude command, so the layout constant no longer needs to carry it. The literal `"claude"` lives at the two non-resurrect call sites.

Three call-site categories:

| Caller | Pass |
|---|---|
| `find_or_create_session` (line 268) | `"claude"` |
| `resurrect_session` (line 234) | `claude --resume <id>` if found, else `"claude"` |
| `apply_layout_creates_named_windows_independent_of_base_index` test (line 1153) | `"claude"` |

The four `resurrect_session_*` tests at lines 1138, 1200, 1236, 1271 do not need direct updates — they call through `mgr.resurrect_session`, which already handles the new argument internally.

### UUID quoting

The resume command is built via `format!("claude --resume {}", id)` and sent through `tmux send-keys`. Claude conversation IDs are hex-and-dash UUIDs (e.g. `d4199215-d987-4117-817d-1d48106b242a`) with no shell metacharacters, so no quoting or escaping is needed at the format-string or send-keys layer.

### Resurrect wiring

In `resurrect_session`, after `wt_path` is resolved and verified to exist:

```rust
let claude_cmd = claude_pane::find_latest_conversation(Path::new(&wt_path))
    .map(|id| format!("claude --resume {}", id))
    .unwrap_or_else(|| "claude".to_string());
apply_layout(&session.name, &wt_path, &claude_cmd);
```

## Testing

### Unit tests for `find_latest_conversation_in` (in `src/claude_pane.rs`)

1. **Latest by mtime when multiple jsonls exist.** Tempdir with `a.jsonl`, `b.jsonl`, `c.jsonl`; bump `b.jsonl`'s mtime; assert returns `"b"`.
2. **None when encoded dir is missing.** Pass a cwd whose encoded form doesn't exist in the fake projects dir.
3. **None when encoded dir is empty of jsonl.** Dir exists, contains only a `subagents/` subdir or a `notes.txt` file.
4. **Ignores non-jsonl files.** Dir with `notes.txt` and `a.jsonl` returns `"a"`.
5. **Encoding correctness.** `/foo/bar` maps to dir name `-foo-bar`.

### Integration test

Skipped. The four existing `resurrect_session_*` tests in `src/manager.rs` (lines 1108–1271) cover the apply_layout + tmux integration. Adding a fifth that asserts the resume-flag path would require a `HOME` override (the `find_latest_conversation` wrapper resolves `dirs::home_dir()`), which is brittle on macOS. Unit-test coverage of the resolver is sufficient.

### Manual verification

After implementation, kill a trellis session that has an active claude conversation and resurrect it from the session list. Confirm the claude window comes up resumed (i.e., shows prior context, not a fresh prompt).

## Out of scope

- Capturing conversation IDs into trellis's SQLite DB during live sessions (Option 2 in brainstorming).
- Repairing the History view's empty state by installing a hook that writes `~/.claude/conversation-index.md`.
- Per-window granularity for ad-hoc claude tabs.
- A user-visible signal when no prior conversation is found.
- Tie-breaking heuristics beyond `read_dir` order when two `.jsonl` files have identical mtimes.
