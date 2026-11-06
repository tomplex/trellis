# Conversation Resume on Session Resurrect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `resurrect_session` revives a dead trellis session, launch its default claude window with `claude --resume <id>` (where `<id>` is the most-recent conversation that ran in that worktree) instead of plain `claude`. Falls back silently to plain `claude` when no prior conversation exists.

**Architecture:** Add a new resolver `find_latest_conversation` in `src/claude_pane.rs` that reads `~/.claude/projects/<encoded-cwd>/` and returns the UUID of the newest `.jsonl` by mtime. Parameterize `apply_layout` in `src/manager.rs` so the first window's claude command is passed in by the caller. `resurrect_session` computes the resume command via the new resolver; `find_or_create_session` continues to pass plain `"claude"`.

**Tech Stack:** Rust 2021, std::fs / std::path, `dirs` crate (already a dependency), `tempfile` (already a dev-dependency).

**Spec:** [`../specs/2026-05-26-conversation-resume-on-resurrect-design.md`](../specs/2026-05-26-conversation-resume-on-resurrect-design.md)

---

## Conventions used in this plan

**Test → fail → impl → pass → commit.** Each behavior change starts with a failing test that pins the intended outcome. When a test would pass without new code (because earlier code already covers it), the plan notes that explicitly so the executor doesn't expect a red bar.

**Build commands:**
- All tests: `cargo test`
- Tests in a single module: `cargo test claude_pane::tests`
- A single test by name: `cargo test <name> -- --nocapture`
- Lint: `cargo clippy --all-targets`
- Build: `cargo build`

**Commit style:** trellis uses conventional prefixes (`feat:`, `fix:`, `refactor:`, `docs:`). Match it.

**Branching:** Work proceeds on `fuzzy-filter-and-worktree-reuse` (the existing branch with 19 commits ahead of main, where the spec was committed at `97ac18a`). No new branch needed — these changes layer onto the same branch.

---

# Phase 1 — Build the conversation resolver

## Task 1: Add resolver skeleton + missing-dir test

Establish the function signature and the simplest test: an absent encoded-cwd dir returns `None`.

**Files:**
- Modify: `src/claude_pane.rs`

- [ ] **Step 1: Add the failing test**

Extend the existing `#[cfg(test)] mod tests` block at the bottom of `src/claude_pane.rs`. Add this test alongside the existing ones:

```rust
#[test]
fn find_latest_returns_none_when_encoded_dir_missing() {
    use tempfile::tempdir;
    let projects = tempdir().unwrap();
    // /nope/here doesn't have a corresponding encoded dir in projects/
    let result = find_latest_conversation_in(projects.path(), Path::new("/nope/here"));
    assert!(result.is_none());
}
```

No new imports are needed — `src/claude_pane.rs` already has `use std::fs;` and `use std::path::Path;`, which cover the test and implementation. (`tempfile` is in dev-dependencies; the test brings it in scope with `use tempfile::tempdir;`.)

- [ ] **Step 2: Run the test to verify it fails (compile error)**

Run: `cargo test claude_pane::tests::find_latest_returns_none_when_encoded_dir_missing`
Expected: compile error — `find_latest_conversation_in` is not defined.

- [ ] **Step 3: Add the function stub**

Add this in `src/claude_pane.rs`, anywhere above the `#[cfg(test)]` block:

```rust
/// Find the most-recent Claude conversation that ran in `cwd`, looking under
/// `projects_dir/<encoded-cwd>/`. Returns the bare UUID (no `.jsonl` extension).
///
/// `projects_dir` is the `~/.claude/projects` location (parameterized for tests).
fn find_latest_conversation_in(projects_dir: &Path, cwd: &Path) -> Option<String> {
    let _ = (projects_dir, cwd);
    None
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test claude_pane::tests::find_latest_returns_none_when_encoded_dir_missing`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add src/claude_pane.rs
git commit -m "feat: add find_latest_conversation_in stub returning None"
```

---

## Task 2: Implement latest-by-mtime selection

Add the read_dir + max-by-mtime logic. The test creates three files, bumps the middle one's mtime, expects that one to win.

**Files:**
- Modify: `src/claude_pane.rs`

- [ ] **Step 1: Add the failing test**

Append to the same `#[cfg(test)] mod tests` block:

```rust
#[test]
fn find_latest_returns_newest_jsonl_by_mtime() {
    use std::fs::File;
    use std::thread::sleep;
    use std::time::Duration;
    use tempfile::tempdir;

    let projects = tempdir().unwrap();
    // cwd we'll fake: /foo/bar -> encoded as "-foo-bar"
    let cwd = Path::new("/foo/bar");
    let encoded = projects.path().join("-foo-bar");
    fs::create_dir_all(&encoded).unwrap();

    // Create three jsonls with distinct mtimes by sleeping between creates.
    // 10ms is enough on every filesystem we care about.
    File::create(encoded.join("aaaa.jsonl")).unwrap();
    sleep(Duration::from_millis(10));
    File::create(encoded.join("bbbb.jsonl")).unwrap();
    sleep(Duration::from_millis(10));
    File::create(encoded.join("cccc.jsonl")).unwrap();
    // Now touch bbbb to make IT the newest by mtime.
    sleep(Duration::from_millis(10));
    File::create(encoded.join("bbbb.jsonl")).unwrap();

    let result = find_latest_conversation_in(projects.path(), cwd);
    assert_eq!(result.as_deref(), Some("bbbb"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test claude_pane::tests::find_latest_returns_newest_jsonl_by_mtime`
Expected: FAIL — assertion fails because the stub returns `None`.

- [ ] **Step 3: Replace the stub with the real implementation**

Replace the body of `find_latest_conversation_in`:

```rust
fn find_latest_conversation_in(projects_dir: &Path, cwd: &Path) -> Option<String> {
    let encoded = cwd.to_string_lossy().replace('/', "-");
    let dir = projects_dir.join(&encoded);
    let entries = fs::read_dir(&dir).ok()?;

    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        match &best {
            Some((best_mtime, _)) if mtime <= *best_mtime => {}
            _ => best = Some((mtime, stem)),
        }
    }
    best.map(|(_, stem)| stem)
}
```

- [ ] **Step 4: Run both tests and verify they pass**

Run: `cargo test claude_pane::tests`
Expected: all `claude_pane` tests pass, including both new ones.

- [ ] **Step 5: Commit**

```bash
git add src/claude_pane.rs
git commit -m "feat: implement find_latest_conversation_in mtime selection"
```

---

## Task 3: Test extension filtering explicitly

The implementation in Task 2 already filters to `.jsonl`. Add a test that pins this behavior so a future change doesn't quietly drop the filter. Honest TDD note: this test will pass on first run.

**Files:**
- Modify: `src/claude_pane.rs`

- [ ] **Step 1: Add the test**

Append to the test module:

```rust
#[test]
fn find_latest_ignores_non_jsonl_files() {
    use std::fs::File;
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;
    use tempfile::tempdir;

    let projects = tempdir().unwrap();
    let cwd = Path::new("/foo/bar");
    let encoded = projects.path().join("-foo-bar");
    fs::create_dir_all(&encoded).unwrap();

    // Create the .jsonl FIRST, then a .txt later. If extension filtering were
    // missing, the .txt's newer mtime would win and the result would be a
    // bare stem ("notes") that doesn't match what we want.
    File::create(encoded.join("real.jsonl")).unwrap();
    sleep(Duration::from_millis(10));
    let mut txt = File::create(encoded.join("notes.txt")).unwrap();
    txt.write_all(b"distractor").unwrap();

    let result = find_latest_conversation_in(projects.path(), cwd);
    assert_eq!(result.as_deref(), Some("real"));
}
```

- [ ] **Step 2: Run the test (expected: passes immediately)**

Run: `cargo test claude_pane::tests::find_latest_ignores_non_jsonl_files`
Expected: PASS. The extension filter added in Task 2 already enforces this.

- [ ] **Step 3: Commit**

```bash
git add src/claude_pane.rs
git commit -m "test: lock in jsonl-only filtering for find_latest_conversation_in"
```

---

## Task 4: Test the empty-of-jsonl case

A dir that exists but contains zero `.jsonl` files (e.g. only a `subagents/` subdir) returns `None`. Like Task 3, the existing impl already handles this — this test pins the contract.

**Files:**
- Modify: `src/claude_pane.rs`

- [ ] **Step 1: Add the test**

```rust
#[test]
fn find_latest_returns_none_when_dir_has_no_jsonl() {
    use tempfile::tempdir;

    let projects = tempdir().unwrap();
    let cwd = Path::new("/foo/bar");
    let encoded = projects.path().join("-foo-bar");
    fs::create_dir_all(encoded.join("subagents")).unwrap();
    // No .jsonl files at the top level.

    let result = find_latest_conversation_in(projects.path(), cwd);
    assert!(result.is_none());
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test claude_pane::tests::find_latest_returns_none_when_dir_has_no_jsonl`
Expected: PASS. `best` stays `None` when the loop finds no `.jsonl` entries.

- [ ] **Step 3: Commit**

```bash
git add src/claude_pane.rs
git commit -m "test: lock in None-when-empty contract for find_latest_conversation_in"
```

---

## Task 5: Test encoding correctness

Pin the `/` → `-` transform. The spec is explicit that `/foo/bar` maps to dir name `-foo-bar`. The Task 2 test implicitly covered this, but an explicit test makes intent obvious.

**Files:**
- Modify: `src/claude_pane.rs`

- [ ] **Step 1: Add the test**

```rust
#[test]
fn find_latest_encoded_cwd_uses_dash_substitution() {
    use std::fs::File;
    use tempfile::tempdir;

    let projects = tempdir().unwrap();
    // Three-segment cwd to make the encoding non-trivial.
    let cwd = Path::new("/a/b/c");
    let expected_dir = projects.path().join("-a-b-c");
    fs::create_dir_all(&expected_dir).unwrap();
    File::create(expected_dir.join("only.jsonl")).unwrap();

    let result = find_latest_conversation_in(projects.path(), cwd);
    assert_eq!(result.as_deref(), Some("only"));
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test claude_pane::tests::find_latest_encoded_cwd_uses_dash_substitution`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/claude_pane.rs
git commit -m "test: lock in encoded-cwd dash substitution for find_latest_conversation_in"
```

---

## Task 6: Add the public wrapper

`find_latest_conversation_in` is private and parameterized. Add a public `find_latest_conversation(cwd)` that resolves the projects dir via `dirs::home_dir()`. No unit test — testing it would require a HOME override, which the spec explicitly skips.

**Files:**
- Modify: `src/claude_pane.rs`

- [ ] **Step 1: Add the public wrapper**

In `src/claude_pane.rs`, just above the private `find_latest_conversation_in`:

```rust
/// Find the most-recent Claude conversation that ran in `cwd`.
///
/// Returns the bare conversation UUID (no `.jsonl` extension), or `None` if
/// no prior conversation exists for this cwd or any I/O error prevents
/// reading the projects directory.
pub fn find_latest_conversation(cwd: &Path) -> Option<String> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    find_latest_conversation_in(&projects_dir, cwd)
}
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles clean.

- [ ] **Step 3: Run the full test suite to confirm nothing regressed**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/claude_pane.rs
git commit -m "feat: add find_latest_conversation public wrapper"
```

---

# Phase 2 — Parameterize apply_layout

## Task 7: Add `claude_cmd` parameter to `apply_layout`

Change `apply_layout`'s signature to take the claude command as a parameter, set `DEFAULT_LAYOUT[0].1 = None`, and update non-resurrect callers to pass `"claude"`.

This task does not change behavior — `resurrect_session` keeps passing `"claude"` until Task 8 wires the lookup. Splitting like this lets the refactor land green.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Update `DEFAULT_LAYOUT`**

In `src/manager.rs` (around line 17), change:

```rust
const DEFAULT_LAYOUT: &[(&str, Option<&str>)] = &[
    ("claude", Some("claude")),
    ("shell", None),
];
```

to:

```rust
const DEFAULT_LAYOUT: &[(&str, Option<&str>)] = &[
    ("claude", None),
    ("shell", None),
];
```

- [ ] **Step 2: Update `apply_layout` signature and body**

Change the function (around line 27) from:

```rust
pub fn apply_layout(session_name: &str, working_dir: &str) {
    let (first_name, first_command) = DEFAULT_LAYOUT[0];
    if let Err(_) = tmux::new_session(session_name, working_dir, Some(first_name)) {
        return;
    }
    if let Some(cmd) = first_command {
        let target = format!("{}:{}", session_name, first_name);
        tmux::send_keys(&target, &[cmd, "Enter"]);
    }
    ...
}
```

to:

```rust
pub fn apply_layout(session_name: &str, working_dir: &str, claude_cmd: &str) {
    let (first_name, _) = DEFAULT_LAYOUT[0];
    if let Err(_) = tmux::new_session(session_name, working_dir, Some(first_name)) {
        return;
    }
    let target = format!("{}:{}", session_name, first_name);
    tmux::send_keys(&target, &[claude_cmd, "Enter"]);

    for &(name, command) in &DEFAULT_LAYOUT[1..] {
        let _ = tmux::new_window(session_name, name, Some(working_dir));
        if let Some(cmd) = command {
            let target = format!("{}:{}", session_name, name);
            tmux::send_keys(&target, &[cmd, "Enter"]);
        }
    }

    let _ = tmux::select_window_by_name(session_name, first_name);
}
```

(The change is: the first window's send-keys is no longer behind `if let Some(cmd) = first_command` — it always sends `claude_cmd`, since callers now always provide one.)

- [ ] **Step 3: Update `find_or_create_session` call site**

Around line 268, change:

```rust
apply_layout(&session_name, wt_path);
```

to:

```rust
apply_layout(&session_name, wt_path, "claude");
```

- [ ] **Step 4: Update `resurrect_session` call site (placeholder)**

Around line 234, change:

```rust
apply_layout(&session.name, &wt_path);
```

to:

```rust
apply_layout(&session.name, &wt_path, "claude");
```

(Task 8 replaces the `"claude"` literal here with the lookup. Doing it in two steps keeps the refactor commit pure and the behavior-change commit minimal.)

- [ ] **Step 5: Update the direct `apply_layout` test (line 1153)**

In `src/manager.rs::apply_layout_creates_named_windows_independent_of_base_index` (around line 1153), change:

```rust
apply_layout(&session_name, &tmp.path().to_string_lossy());
```

to:

```rust
apply_layout(&session_name, &tmp.path().to_string_lossy(), "claude");
```

- [ ] **Step 6: Build and run the full test suite**

Run: `cargo test`
Expected: every test passes. The four `resurrect_session_*` tests (lines 1138, 1200, 1236, 1271) go through `mgr.resurrect_session` and require no direct update — they pick up the new arg through that path.

- [ ] **Step 7: Lint**

Run: `cargo clippy --all-targets`
Expected: no new warnings on touched code.

- [ ] **Step 8: Commit**

```bash
git add src/manager.rs
git commit -m "refactor: parameterize apply_layout's claude command"
```

---

# Phase 3 — Wire resume into resurrect

## Task 8: Look up and pass the resume command in `resurrect_session`

Replace the placeholder `"claude"` in `resurrect_session` with a lookup via `claude_pane::find_latest_conversation`.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Update `resurrect_session` to compute `claude_cmd`**

In `src/manager.rs::resurrect_session` (around line 230-234), replace:

```rust
        if !Path::new(&wt_path).exists() {
            return Err(format!("path '{}' no longer exists", wt_path));
        }

        apply_layout(&session.name, &wt_path, "claude");
```

with:

```rust
        if !Path::new(&wt_path).exists() {
            return Err(format!("path '{}' no longer exists", wt_path));
        }

        let claude_cmd = crate::claude_pane::find_latest_conversation(Path::new(&wt_path))
            .map(|id| format!("claude --resume {}", id))
            .unwrap_or_else(|| "claude".to_string());
        apply_layout(&session.name, &wt_path, &claude_cmd);
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles clean. (If `crate::claude_pane` isn't already in scope at the top of `manager.rs`, the fully-qualified path used above resolves it without a new `use` statement. Confirm there's no compile error before continuing.)

- [ ] **Step 3: Run the test suite**

Run: `cargo test`
Expected: all tests pass. The existing `resurrect_session_*` tests don't exercise a populated `~/.claude/projects` directory, so they'll continue to hit the `None` branch and pass plain `"claude"` to `apply_layout` — same behavior as before this change.

- [ ] **Step 4: Lint**

Run: `cargo clippy --all-targets`
Expected: no new warnings.

- [ ] **Step 5: Commit**

```bash
git add src/manager.rs
git commit -m "feat: resume prior claude conversation on session resurrect"
```

---

# Phase 4 — Manual smoke test

## Task 9: Verify the end-to-end behavior

Unit tests cover the resolver. Apply_layout's call sites compile. The real test is: does resurrecting a session actually `--resume` the prior conversation? Verify by hand.

**Files:** none (manual)

- [ ] **Step 1: Install the new binary**

```bash
cargo install --path .
```

Expected: `trellis` binary installed to `~/.cargo/bin/trellis`.

- [ ] **Step 2: Identify a target session**

In your tmux setup, pick a live trellis session that has an active Claude Code conversation. Note its session name and worktree path.

Sanity-check that Claude is writing a `.jsonl` for it:

```bash
WORKTREE="<paste the worktree path>"
ls -lt ~/.claude/projects/$(echo "$WORKTREE" | tr '/' '-')/*.jsonl | head -3
```

Expected: at least one `.jsonl` file, recently modified.

- [ ] **Step 3: Kill the tmux session**

```bash
tmux kill-session -t <session-name>
```

- [ ] **Step 4: Resurrect via trellis**

Open the trellis popup (your existing `Alt+s` binding). The dead session row should show. Press `Enter` on it.

- [ ] **Step 5: Inspect the claude window**

Tmux should attach to the session. The `claude` window's first input line should be:

```
claude --resume <uuid>
```

(visible briefly before claude starts) and Claude Code should come up with the prior conversation's context (the most recent message visible, history scrollable).

- [ ] **Step 6: Negative case**

Repeat with a session whose worktree has *no* matching projects dir (e.g., a freshly-created worktree no one's used Claude in yet). Resurrect should launch plain `claude`, identical to today's behavior.

- [ ] **Step 7: Done**

If both cases work, the feature is live. If either fails, debug before reporting complete. Common gotchas:
- Wrong encoded path (verify the worktree path matches what's under `~/.claude/projects/`).
- Claude's `--resume` rejects the UUID (run `claude --resume <uuid>` manually in the worktree directory to confirm Claude itself accepts it).
- `tmux send-keys` interleaving issues (very unlikely with a UUID, but worth ruling out).

---

# Out of scope

The following are intentionally not part of this plan (see spec section "Out of scope"):

- Capturing conversation IDs into trellis's SQLite DB during live sessions (the speculative "Option 2" path from brainstorming).
- Repairing the History view by installing a hook that writes `~/.claude/conversation-index.md`.
- Per-window granularity for ad-hoc claude tabs spawned via the actions menu.
- A user-visible signal (status line, dialog) when no prior conversation is found.
- Tie-breaking when two `.jsonl` files share an mtime down to the nanosecond.
