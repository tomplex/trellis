# Fuzzy Filter and Worktree Reuse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace substring filtering with fuzzy matching in the four picker screens that don't already use it, and refactor `Manager`'s three worktree-creating flows onto unified `find_or_create_*` helpers so existing worktrees and sessions are reused instead of erroring.

**Architecture:** Phase A is per-screen inline swaps from `.to_lowercase().contains()` to `fuzzy_match` + score sort, modeled on `src/tui/session_list.rs`. Phase B introduces two new private methods on `Manager` (`find_or_create_worktree`, `find_or_create_session`) and one new public helper in `git.rs` (`prune_worktrees`); the existing flows `create_session`, `add_tab`, and `checkout_and_review` are reduced to thin wrappers over those helpers.

**Tech Stack:** Rust 2021, ratatui 0.29, rusqlite, plus a new `tempfile` dev-dependency for git-repo tests.

**Spec:** [`../specs/2026-05-19-fuzzy-filter-and-worktree-reuse-design.md`](../specs/2026-05-19-fuzzy-filter-and-worktree-reuse-design.md)

---

## Conventions used in this plan

**Test → fail → impl → pass → commit.** Every behavior change starts with a failing test that pins the intended outcome.

**Build commands:**
- All tests: `cargo test`
- A single test by name: `cargo test <name> -- --nocapture`
- Lint: `cargo clippy --all-targets`
- Build: `cargo build`

**Commit style:** trellis uses conventional prefixes (`feat:`, `fix:`, `refactor:`, `docs:`). Match it.

**Branching:** All work goes on a feature branch off `main`. Create it as your first step (Task 0).

---

## Task 0: Create feature branch

**Files:** none (git state only)

- [ ] **Step 1: Create and check out branch**

```bash
git checkout main
git pull --ff-only
git checkout -b fuzzy-filter-and-worktree-reuse
```

- [ ] **Step 2: Confirm clean tree**

Run: `git status`
Expected: `On branch fuzzy-filter-and-worktree-reuse` and `nothing to commit, working tree clean`.

---

# Phase A — Fuzzy filtering across pickers

## Task A1: Fuzzy filter in `new_session.rs`

`new_session.rs` has three pickers (repos, branches, subsystems) inside `apply_filter`. Convert each to use `fuzzy_match` and sort by score with alphabetic tiebreak.

**Files:**
- Modify: `src/tui/new_session.rs`

- [ ] **Step 1: Add the failing test**

At the bottom of `src/tui/new_session.rs`, append (or extend the existing `#[cfg(test)] mod tests` block if one exists):

```rust
#[cfg(test)]
mod tests {
    use crate::fuzzy::fuzzy_match;

    /// Helper that mirrors what the picker's filter will do post-change.
    fn rank(query: &str, items: &[&str]) -> Vec<String> {
        let mut scored: Vec<(String, i32)> = items
            .iter()
            .filter_map(|s| fuzzy_match(query, s).map(|score| (s.to_string(), score)))
            .collect();
        scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        scored.into_iter().map(|(s, _)| s).collect()
    }

    #[test]
    fn filter_ranks_subsequence_above_substring_only() {
        // "feat" is a subsequence of both; "feature" gets the higher (worse)
        // score because of the gap, "feat" matches consecutively.
        let items = ["feat", "feature", "unrelated"];
        let ranked = rank("ft", &items);
        assert_eq!(ranked, vec!["feat".to_string(), "feature".to_string()]);
    }

    #[test]
    fn filter_alphabetic_tiebreak_when_scores_equal() {
        // Both candidates have the query char 'a' at position 0 -> identical
        // score (0) from fuzzy_match -> alphabetical tiebreak picks "aaa" first.
        let items = ["aaa", "abc"];
        let ranked = rank("a", &items);
        assert_eq!(ranked, vec!["aaa".to_string(), "abc".to_string()]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib tui::new_session::tests`
Expected: tests don't fail yet because they use `fuzzy_match` directly, not the picker's `apply_filter`. The test is forward-looking — it documents the contract. Move to Step 3.

(Note: this test exercises the same algorithm `apply_filter` will use. The "real" test that filter results match this is done by visual inspection after the impl, because `apply_filter` requires a full `NewSessionScreen` with `Manager` state to construct.)

- [ ] **Step 3: Rewrite `apply_filter` to use fuzzy ranking**

Replace the body of `apply_filter` (currently at `src/tui/new_session.rs:267-306`) with:

```rust
fn apply_filter(&mut self) {
    let query = self.filter_input.clone();
    match self.step {
        Step::PickRepo => {
            let mut scored: Vec<((String, String), i32)> = self
                .dev_dirs
                .iter()
                .filter_map(|(n, p)| {
                    let best = [n.as_str(), p.as_str()]
                        .iter()
                        .filter_map(|c| crate::fuzzy::fuzzy_match(&query, c))
                        .min();
                    best.map(|score| ((n.clone(), p.clone()), score))
                })
                .collect();
            scored.sort_by(|a, b| {
                a.1.cmp(&b.1).then_with(|| a.0 .0.to_lowercase().cmp(&b.0 .0.to_lowercase()))
            });
            self.filtered_dirs = scored.into_iter().map(|((n, p), _)| (n, p)).collect();
            self.repo_list_state = ratatui::widgets::ListState::default();
            if !self.filtered_dirs.is_empty() {
                self.repo_list_state.select(Some(0));
            }
        }
        Step::PickBranch => {
            let mut scored: Vec<(String, i32)> = self
                .branches
                .iter()
                .filter_map(|b| crate::fuzzy::fuzzy_match(&query, b).map(|s| (b.clone(), s)))
                .collect();
            scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            self.filtered_branches = scored.into_iter().map(|(b, _)| b).collect();
            self.branch_list_state = ratatui::widgets::ListState::default();
            if !self.filtered_branches.is_empty() {
                self.branch_list_state.select(Some(0));
            }
        }
        Step::PickSubsystem => {
            let mut scored: Vec<(String, i32)> = self
                .subsystems
                .iter()
                .filter_map(|s| crate::fuzzy::fuzzy_match(&query, s).map(|sc| (s.clone(), sc)))
                .collect();
            scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            self.filtered_subsystems = scored.into_iter().map(|(s, _)| s).collect();
            self.subsystem_list_state = ratatui::widgets::ListState::default();
            // Always have root at index 0
            self.subsystem_list_state.select(Some(0));
        }
    }
}
```

Notes:
- Drop the manual `.to_lowercase()` — `fuzzy_match` is already case-insensitive.
- Use fully-qualified `ratatui::widgets::ListState::default()` only if `ListState` isn't already in scope at this file's top. It is — check the existing `use` statements and keep the call style consistent with the surrounding code (likely just `ListState::default()`).

- [ ] **Step 4: Verify build + tests pass**

Run: `cargo build && cargo test --lib`
Expected: clean build, all tests pass.

- [ ] **Step 5: Smoke-test the picker manually**

```bash
cargo install --path .
# In a tmux session inside WezTerm:
# Alt+s to open trellis, n to create a new session,
# type a few characters and verify that:
#  - results that match consecutively rank above results that match with gaps
#  - identical-score results appear in alphabetical order
```

- [ ] **Step 6: Commit**

```bash
git add src/tui/new_session.rs
git commit -m "feat: fuzzy filter in new-session picker"
```

---

## Task A2: Fuzzy filter in `edit_branch.rs`

The filter logic lives in `rebuild_display` (not `apply_filter`).

**Files:**
- Modify: `src/tui/edit_branch.rs`

- [ ] **Step 1: Rewrite the filter block in `rebuild_display`**

Replace `src/tui/edit_branch.rs:87-97` (the `let query = ... let filtered: Vec<String> = ...` block) with:

```rust
let filtered: Vec<String> = if self.filter.is_empty() {
    self.all_branches.clone()
} else {
    let mut scored: Vec<(String, i32)> = self
        .all_branches
        .iter()
        .filter_map(|b| crate::fuzzy::fuzzy_match(&self.filter, b).map(|s| (b.clone(), s)))
        .collect();
    scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    scored.into_iter().map(|(b, _)| b).collect()
};
```

Leave the rest of `rebuild_display` (the "new branch" affordance, selection reset) unchanged.

- [ ] **Step 2: Verify build + tests**

Run: `cargo build && cargo test --lib`
Expected: clean build, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/tui/edit_branch.rs
git commit -m "feat: fuzzy filter in edit-branch picker"
```

---

## Task A3: Fuzzy filter in `adopt_session.rs`

Same pattern as Task A1. Two pickers: repos (filter on `name` + `path`), branches (filter on branch string).

**Files:**
- Modify: `src/tui/adopt_session.rs`

- [ ] **Step 1: Rewrite `apply_filter`**

Replace the body of `apply_filter` (at `src/tui/adopt_session.rs:163-195`) with:

```rust
fn apply_filter(&mut self) {
    let query = self.filter_input.clone();
    match self.step {
        Step::PickRepo => {
            let mut scored: Vec<(usize, i32)> = self
                .repos
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    let best = [r.name.as_str(), r.path.as_str()]
                        .iter()
                        .filter_map(|c| crate::fuzzy::fuzzy_match(&query, c))
                        .min();
                    best.map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| {
                a.1.cmp(&b.1).then_with(|| {
                    self.repos[a.0].name.to_lowercase().cmp(&self.repos[b.0].name.to_lowercase())
                })
            });
            self.filtered_repos = scored.into_iter().map(|(i, _)| i).collect();
            self.repo_list_state = ListState::default();
            if !self.filtered_repos.is_empty() {
                self.repo_list_state.select(Some(0));
            }
        }
        Step::PickBranch => {
            let mut scored: Vec<(String, i32)> = self
                .branches
                .iter()
                .filter_map(|b| crate::fuzzy::fuzzy_match(&query, b).map(|s| (b.clone(), s)))
                .collect();
            scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            self.filtered_branches = scored.into_iter().map(|(b, _)| b).collect();
            self.branch_list_state = ListState::default();
            if !self.filtered_branches.is_empty() {
                self.branch_list_state.select(Some(0));
            }
        }
    }
}
```

- [ ] **Step 2: Verify build + tests**

Run: `cargo build && cargo test --lib`
Expected: clean build, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/tui/adopt_session.rs
git commit -m "feat: fuzzy filter in adopt-session picker"
```

---

## Task A4: Fuzzy filter in `history.rs`

`history.rs` filters across four fields per row (`date`, `project`, `branch`, `summary`). Take the best score across the four fields. Break ties on `(project, branch, date)` lowercased.

**Files:**
- Modify: `src/tui/history.rs`

- [ ] **Step 1: Rewrite the filter block in `rebuild`**

Replace `src/tui/history.rs:95-109` (the `let filtered: Vec<Conversation> = ...` block) with:

```rust
let filtered: Vec<Conversation> = if self.filter.is_empty() {
    entries.to_vec()
} else {
    let q = &self.filter;
    let mut scored: Vec<(Conversation, i32)> = entries
        .iter()
        .filter_map(|e| {
            // `e.summary()` returns &str — Conversation::summary in src/models.rs.
            let candidates: [&str; 4] = [&e.date, &e.project, &e.branch, e.summary()];
            let best = candidates
                .iter()
                .filter_map(|c| crate::fuzzy::fuzzy_match(q, c))
                .min();
            best.map(|score| (e.clone(), score))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.0.project.to_lowercase().cmp(&b.0.project.to_lowercase()))
            .then_with(|| a.0.branch.to_lowercase().cmp(&b.0.branch.to_lowercase()))
            .then_with(|| a.0.date.cmp(&b.0.date))
    });
    scored.into_iter().map(|(e, _)| e).collect()
};
```

Note: the existing code re-sorts `filtered` afterwards based on `self.sort_by` (Date / Project / Branch / Summary). Keep that downstream block untouched — when the user has an active sort column selected, that overrides the fuzzy score ordering. That's existing behavior; we're only changing how `filtered` is built, not how it's subsequently sorted.

- [ ] **Step 2: Verify build + tests**

Run: `cargo build && cargo test --lib`
Expected: clean build, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/tui/history.rs
git commit -m "feat: fuzzy filter in history picker"
```

---

# Phase B — Worktree reuse

## Task B0: Add `tempfile` dev-dependency

The new helper tests need real git repos in temp dirs.

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Append dev-dependencies section**

Add to `Cargo.toml` after the `[dependencies]` block:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Verify Cargo accepts the new dep**

Run: `cargo build --tests`
Expected: tempfile downloads and compiles; no errors.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tempfile dev-dependency for worktree tests"
```

---

## Task B1: `git::prune_worktrees` and `git::add_worktree_existing_branch` helpers

Two helpers:
- `prune_worktrees`: thin wrapper for `git worktree prune` — used to clear stale admin entries when a worktree dir was deleted manually.
- `add_worktree_existing_branch`: attaches a worktree to a *branch that already exists locally*. `create_worktree` always uses `-b`, which errors on existing branches; we need the no-`-b` form for the recovery path.

**Files:**
- Modify: `src/git.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/git.rs` (add a `#[cfg(test)] mod tests` block; none exists currently in `git.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn init_repo(dir: &std::path::Path) {
        Command::new("git").args(["init", "-q"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.name", "t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["commit", "--allow-empty", "-qm", "init"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["branch", "-M", "main"]).current_dir(dir).output().unwrap();
    }

    #[test]
    fn prune_removes_stale_worktree_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        Command::new("git")
            .args(["worktree", "add", "-b", "feat", wt.to_str().unwrap(), "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        fs::remove_dir_all(&wt).unwrap();

        let before = list_worktrees(repo.to_str().unwrap()).unwrap();
        assert!(before.iter().any(|w| w.path == wt.to_string_lossy()));

        prune_worktrees(repo.to_str().unwrap()).unwrap();

        let after = list_worktrees(repo.to_str().unwrap()).unwrap();
        assert!(!after.iter().any(|w| w.path == wt.to_string_lossy()));
    }

    #[test]
    fn add_worktree_existing_branch_attaches_without_creating_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        // Create branch separately, then attach a worktree without -b.
        Command::new("git").args(["branch", "feat-existing"]).current_dir(&repo).output().unwrap();
        let wt = tmp.path().join("wt");

        add_worktree_existing_branch(repo.to_str().unwrap(), wt.to_str().unwrap(), "feat-existing").unwrap();

        let listed = list_worktrees(repo.to_str().unwrap()).unwrap();
        assert!(listed.iter().any(|w| w.branch.as_deref() == Some("feat-existing")));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib git::tests`
Expected: FAIL with "cannot find function `prune_worktrees`" / "cannot find function `add_worktree_existing_branch`".

- [ ] **Step 3: Implement both helpers**

Add to `src/git.rs` (anywhere in the public-fn region, e.g. after `remove_worktree`):

```rust
pub fn prune_worktrees(repo_path: &str) -> Result<(), GitError> {
    let output = run(&["worktree", "prune"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to prune worktrees in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Attach a worktree to a branch that already exists locally.
/// Unlike `create_worktree`, this does not use `-b`, so it does not require
/// the branch to be new.
pub fn add_worktree_existing_branch(
    repo_path: &str,
    worktree_path: &str,
    branch: &str,
) -> Result<(), GitError> {
    let output = run(
        &["worktree", "add", worktree_path, branch],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to attach worktree at '{}' to branch '{}': {}",
            worktree_path,
            branch,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
```

- [ ] **Step 4: Verify tests pass**

Run: `cargo test --lib git::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/git.rs
git commit -m "feat: add git::prune_worktrees and add_worktree_existing_branch helpers"
```

---

## Task B2: `Manager::find_or_create_worktree`

Resolves the on-disk path for `(repo, branch)`. If git already has a worktree for that branch, return its recorded path. If the dir was deleted manually, prune and recreate. Otherwise create fresh at the trellis-convention path.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Write the failing tests**

Add a `#[cfg(test)] mod tests` block at the bottom of `src/manager.rs` (none exists currently in this file). The tests do not need a full `Manager` — they exercise the *behavior* through a thin `find_or_create_worktree` API that we'll wire to take a `repo_path` + `worktrees_dir` so it's testable without a sqlite Connection. The actual method on `Manager` will be a one-line adapter.

Two functions: a free function `resolve_or_create_worktree` that does the work (testable), and the `Manager` method that just calls it with the manager's own `worktrees_dir()` + repo metadata.

Append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo(dir: &Path) {
        Command::new("git").args(["init", "-q"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.name", "t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["commit", "--allow-empty", "-qm", "init"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["branch", "-M", "main"]).current_dir(dir).output().unwrap();
    }

    fn make_repo(tmp: &Path) -> (String, String) {
        let repo = tmp.join("repo");
        let worktrees = tmp.join("worktrees");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&worktrees).unwrap();
        init_repo(&repo);
        (repo.to_string_lossy().to_string(), worktrees.to_string_lossy().to_string())
    }

    #[test]
    fn resolve_creates_when_no_existing_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let path = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main")
            .unwrap();
        assert!(Path::new(&path).exists());
        assert!(path.ends_with("feat-x"));
    }

    #[test]
    fn resolve_reuses_existing_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let first = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main")
            .unwrap();
        let second = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main")
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn resolve_recovers_from_prunable_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let first = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main")
            .unwrap();
        // Nuke the dir behind git's back to make it prunable.
        fs::remove_dir_all(&first).unwrap();
        let second = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main")
            .unwrap();
        assert!(Path::new(&second).exists());
    }

    #[test]
    fn resolve_finds_worktree_at_nonconventional_path() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        // User created a worktree at an unusual path outside our convention.
        let unusual = tmp.path().join("unusual-spot");
        Command::new("git")
            .args(["worktree", "add", "-b", "feat-y", unusual.to_str().unwrap(), "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        // Now ask trellis to find/create for feat-y — should return the unusual path.
        let found = resolve_or_create_worktree(&repo, "myrepo", "feat-y", &wts, "main").unwrap();
        assert_eq!(found, unusual.to_string_lossy().to_string());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib manager::tests`
Expected: FAIL with "cannot find function `resolve_or_create_worktree` in this scope" (or similar).

- [ ] **Step 3: Implement the free function and Manager method**

Add to `src/manager.rs`, somewhere before the `impl Manager` block (e.g. just below `apply_layout`):

```rust
/// Find or create the on-disk worktree for (repo, branch).
///
/// Git's `worktree list` is the source of truth. If a worktree for `branch`
/// already exists, returns its recorded path (even if outside `worktrees_dir`).
/// If git considers a worktree prunable (dir gone), prunes and reattaches the
/// worktree to the still-existing branch. Otherwise creates fresh at the
/// trellis-convention path, branching from `start_point`.
///
/// `start_point` is only consulted when creating a NEW local branch. Callers
/// pass what they want the branch rooted at:
/// - `create_session` (n): `repo.default_branch` (e.g. `"main"`)
/// - `add_tab` (t): `session.base_branch` (the session's root branch)
/// - `checkout_and_review` (R): `format!("origin/{}", branch)` (the PR's HEAD)
pub fn resolve_or_create_worktree(
    repo_path: &str,
    repo_name: &str,
    branch: &str,
    worktrees_dir: &str,
    start_point: &str,
) -> Result<String, String> {
    let mut needs_prune = false;
    let existing = git::list_worktrees(repo_path).map_err(|e| e.to_string())?;
    for wt in &existing {
        if wt.branch.as_deref() == Some(branch) {
            if !Path::new(&wt.path).exists() {
                needs_prune = true;
                break;
            }
            return Ok(wt.path.clone());
        }
    }

    if needs_prune {
        let _ = git::prune_worktrees(repo_path);
    }

    let wt_path = Path::new(worktrees_dir)
        .join(repo_name)
        .join(branch.replace('/', "-"))
        .to_string_lossy()
        .to_string();

    // Pick between `git worktree add -b <branch> <path> <start>` (new branch)
    // and `git worktree add <path> <branch>` (existing branch). After a prune,
    // the local branch ref still exists, so the existing-branch path is taken.
    let branches = git::list_branches(repo_path).map_err(|e| e.to_string())?;
    if branches.iter().any(|b| b == branch) {
        git::add_worktree_existing_branch(repo_path, &wt_path, branch)
            .map_err(|e| format!("Failed to attach worktree: {}", e))?;
    } else {
        git::create_worktree(repo_path, &wt_path, branch, start_point)
            .map_err(|e| format!("Failed to create worktree: {}", e))?;
    }
    Ok(wt_path)
}
```

Then inside `impl Manager`, add the adapter method (after `worktree_path` or alongside the other private helpers — somewhere logical):

```rust
fn find_or_create_worktree(
    &self,
    repo: &Repo,
    branch: &str,
    start_point: &str,
) -> Result<String, String> {
    resolve_or_create_worktree(
        &repo.path,
        &repo.name,
        branch,
        &self.worktrees_dir().to_string_lossy(),
        start_point,
    )
}
```

Note: `start_point` is required, not optional. Each caller knows what it wants the new branch rooted at — making it explicit prevents the kind of silent-default bug that the original `checkout_and_review`'s `origin/<branch>` would mask.

- [ ] **Step 4: Verify tests pass**

Run: `cargo test --lib manager::tests`
Expected: all four `resolve_*` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/manager.rs
git commit -m "feat: add find_or_create_worktree helper"
```

---

## Task B3: `Manager::find_or_create_session`

Resolves the trellis session for `(repo, branch)`. If a DB row exists AND the underlying tmux session is alive, return it. If the DB row is stale, drop it. Otherwise add the DB row, create the tmux session via `apply_layout`.

Unlike B2, this method needs the `Manager`'s sqlite `Connection`, so the tests build a real `Manager` against an in-memory sqlite DB. tmux can't be mocked easily (it shells out), so the test for "session is alive" path verifies via `tmux::session_exists` only when tmux is available; otherwise it asserts the create-fresh path.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Write the failing tests**

Add these to the existing `mod tests` in `src/manager.rs`. The tests call `apply_layout`, which shells out to tmux — to keep them robust across runs and not litter the dev's tmux server, use unique session names per process and kill them in a `Drop` guard.

```rust
fn make_manager() -> Manager {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::init_schema(&conn);  // see Step 1a below
    Manager::new(conn)
}

/// Cleans up a tmux session at end of test (even on panic).
struct SessionGuard(String);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = crate::tmux::kill_session(&self.0);
    }
}

/// Process-unique branch name. tmux::sanitize_session_name leaves `-` and
/// alphanumerics alone, so the sanitized version is identical.
fn unique_branch(suffix: &str) -> String {
    format!("trellistest-{}-{}", std::process::id(), suffix)
}

#[test]
fn find_or_create_session_inserts_when_missing() {
    let tmp = tempdir().unwrap();
    let (repo_path, _) = make_repo(tmp.path());
    let mgr = make_manager();

    let repo = mgr.get_or_create_repo(&repo_path);
    let branch = unique_branch("fresh");
    let _guard = SessionGuard(crate::tmux::sanitize_session_name(&branch));
    let wt_path = tmp.path().join("wt").to_string_lossy().to_string();

    let session = mgr.find_or_create_session(&repo, &branch, &wt_path).unwrap();
    assert_eq!(session.base_branch, branch);
    assert!(crate::db::get_session_by_name(&mgr.conn, &session.name).is_some());
}

#[test]
fn find_or_create_session_drops_stale_db_row_then_inserts() {
    let tmp = tempdir().unwrap();
    let (repo_path, _) = make_repo(tmp.path());
    let mgr = make_manager();

    let repo = mgr.get_or_create_repo(&repo_path);
    let branch = unique_branch("stale");
    let session_name = crate::tmux::sanitize_session_name(&branch);
    let _guard = SessionGuard(session_name.clone());

    // Pre-insert a "stale" DB row. The corresponding tmux session does not
    // exist yet (unique name + Drop cleanup guarantees this across runs).
    crate::db::add_session(&mgr.conn, &crate::models::Session {
        id: None,
        name: session_name.clone(),
        repo_id: repo.id.unwrap(),
        base_branch: branch.clone(),
        created_at: crate::utils::utc_now(),
        last_selected_at: None,
    });
    assert!(crate::db::get_session_by_name(&mgr.conn, &session_name).is_some());
    assert!(!crate::tmux::session_exists(&session_name));

    let wt_path = tmp.path().join("wt").to_string_lossy().to_string();
    let fresh = mgr.find_or_create_session(&repo, &branch, &wt_path).unwrap();
    assert_eq!(fresh.name, session_name);
    let rows: Vec<_> = crate::db::get_sessions(&mgr.conn)
        .into_iter()
        .filter(|s| s.name == session_name)
        .collect();
    assert_eq!(rows.len(), 1);
}
```

Note on `sanitize_session_name`: it only replaces `.` and `:` with `-`. Slashes pass through unchanged. The `unique_branch` helper above uses only `-` and alphanumerics, so the sanitized form equals the input.

- [ ] **Step 1a: Extract `db::init_schema` from `db::init_db`**

The test above uses `crate::db::init_schema(&conn)`. This function doesn't exist yet — `init_db` does both connection-opening AND schema-application in one. Extract the schema part so an in-memory Connection can be initialized for tests.

Concretely:
1. Open `src/db.rs`. Find `init_db(db_path: &Path) -> Connection` (line 6).
2. Identify the CREATE TABLE / migration logic in its body.
3. Extract that logic into:
   ```rust
   pub fn init_schema(conn: &Connection) {
       // (paste the schema-application body here, unchanged)
   }
   ```
4. Have `init_db` open the connection, call `init_schema(&conn)`, and return the connection. The existing `db.rs` test block at line ~304 may also benefit from being switched to use this; if it builds the schema by hand, point it at `init_schema` so there's a single source of truth.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib manager::tests`
Expected: FAIL with "no method named `find_or_create_session` found for struct `Manager`" (or similar).

- [ ] **Step 3: Implement `find_or_create_session`**

Inside `impl Manager`, add (next to `find_or_create_worktree`):

```rust
fn find_or_create_session(&self, repo: &Repo, branch: &str, wt_path: &str) -> Result<Session, String> {
    let session_name = tmux::sanitize_session_name(branch);

    // Existing DB row?
    if let Some(existing) = self.get_session_by_name(&session_name) {
        if existing.repo_id == repo.id.unwrap() && tmux::session_exists(&session_name) {
            return Ok(existing);
        }
        // Stale row — drop it.
        if let Some(id) = existing.id {
            db::delete_session(&self.conn, id);
        }
    }

    // Create fresh: DB row, tmux session via apply_layout, optional worktree DB row.
    let session = db::add_session(
        &self.conn,
        &Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.to_string(),
            created_at: utc_now(),
            last_selected_at: None,
        },
    );
    apply_layout(&session_name, wt_path);

    // Record the worktree in the DB if it's not the repo root.
    if wt_path != repo.path {
        if db::get_worktree_by_path(&self.conn, wt_path).is_none() {
            db::add_worktree(
                &self.conn,
                &Worktree {
                    id: None,
                    repo_id: repo.id.unwrap(),
                    path: wt_path.to_string(),
                    branch: branch.to_string(),
                    session_id: session.id,
                    tmux_window: None,
                    created_at: utc_now(),
                },
            );
        }
    }

    Ok(session)
}
```

`get_session_by_name` is already a method on `Manager` at `src/manager.rs:778` (`pub`). `db::get_worktree_by_path` exists at `src/db.rs:267`.

- [ ] **Step 4: Verify tests pass**

Run: `cargo test --lib manager::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/db.rs src/manager.rs
git commit -m "feat: add find_or_create_session helper"
```

---

## Task B4: Refactor `create_session` onto helpers

The new-session-wizard call path. Today it does worktree creation + DB session + tmux session inline.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Rewrite `create_session`**

Replace the body of `create_session` (currently `src/manager.rs:159-225`) with:

```rust
pub fn create_session(
    &self,
    repo_path: &str,
    base_branch: &str,
    session_name: &str,
    subdirectory: Option<&str>,
) -> Result<Session, String> {
    let repo = self.get_or_create_repo(repo_path);

    // Spec semantic: "n on branch X when X already has a session — route to
    // the existing session." Consult by branch BEFORE honoring the user-typed
    // session name. The wizard's auto_session_name disambiguates to
    // "feat-x-2", "feat-x-3", etc. when a session for the branch exists; that
    // disambiguation is what we want to bypass here.
    let by_branch: Vec<Session> = db::get_sessions(&self.conn)
        .into_iter()
        .filter(|s| {
            s.repo_id == repo.id.unwrap()
                && s.base_branch == base_branch
        })
        .collect();
    for s in by_branch {
        if tmux::session_exists(&s.name) {
            return Ok(s);
        }
        // Dead tmux session — drop stale row and continue.
        if let Some(id) = s.id {
            db::delete_session(&self.conn, id);
        }
    }

    // No live session for this branch. Resolve the worktree (find existing or create).
    let wt_path = if base_branch == repo.default_branch {
        repo.path.clone()
    } else {
        // Fetch latest before resolving worktree (non-fatal — stale is acceptable).
        let _ = git::fetch_and_pull(repo_path, &repo.default_branch);
        // New branches root at the repo's default branch.
        self.find_or_create_worktree(&repo, base_branch, &repo.default_branch)?
    };

    // Honor the user-typed session name (or fall back to sanitized branch).
    let chosen_name = if session_name.is_empty() {
        tmux::sanitize_session_name(base_branch)
    } else {
        tmux::sanitize_session_name(session_name)
    };

    let effective_dir = match subdirectory {
        Some(sub) => Path::new(&wt_path).join(sub).to_string_lossy().to_string(),
        None => wt_path.clone(),
    };

    let session = db::add_session(
        &self.conn,
        &Session {
            id: None,
            name: chosen_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: base_branch.to_string(),
            created_at: utc_now(),
            last_selected_at: None,
        },
    );
    apply_layout(&chosen_name, &effective_dir);

    if wt_path != repo.path && db::get_worktree_by_path(&self.conn, &wt_path).is_none() {
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id: repo.id.unwrap(),
                path: wt_path.clone(),
                branch: base_branch.to_string(),
                session_id: session.id,
                tmux_window: None,
                created_at: utc_now(),
            },
        );
    }

    Ok(session)
}
```

Notes:
- The early by-branch lookup is the spec's "route to where it lives" behavior. It runs unconditionally — if a live session exists for the (repo, branch), the user's typed session name is ignored and they land in the existing session.
- The post-lookup path always honors the user-typed name. No conditional branching on whether the name matches the default.
- `find_or_create_session` (Task B3) is not used in this refactor — its main consumer is `checkout_and_review`, which always uses the default sanitized branch name.

- [ ] **Step 2: Verify build + tests**

Run: `cargo build && cargo test --lib`
Expected: clean build, all existing tests pass (the manager tests from B2/B3 still pass, plus any test of `create_session` that existed before).

- [ ] **Step 3: Smoke-test manually**

```bash
cargo install --path .
# In tmux + WezTerm:
# Alt+s → n → pick a branch you already have a worktree for.
# Expected: routes to the existing session (no "worktree already exists" error).
```

- [ ] **Step 4: Commit**

```bash
git add src/manager.rs
git commit -m "refactor: route create_session through find_or_create_* helpers"
```

---

## Task B5: Refactor `add_tab` onto helpers

The new-tab (`t`) call path. Today it hard-fails when the worktree exists.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Rewrite `add_tab`**

Replace the body of `add_tab` (currently `src/manager.rs:367-395`) with:

```rust
pub fn add_tab(&self, session_id: i64, branch_name: &str) -> Result<Worktree, String> {
    let session = self
        .get_session_by_id(session_id)
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    let repo = self
        .get_repo_by_id(session.repo_id)
        .ok_or_else(|| format!("Repo {} not found", session.repo_id))?;

    // New branches in a tab root at the session's base branch (preserves existing behavior).
    let wt_path = self.find_or_create_worktree(&repo, branch_name, &session.base_branch)?;

    // Attach a new tmux window in the existing session, pointed at the worktree.
    let _ = tmux::new_window(&session.name, branch_name, Some(&wt_path));

    // Record the worktree in the DB if we haven't already.
    let worktree = if let Some(existing) = db::get_worktree_by_path(&self.conn, &wt_path) {
        existing
    } else {
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id: repo.id.unwrap(),
                path: wt_path,
                branch: branch_name.to_string(),
                session_id: Some(session_id),
                tmux_window: None,
                created_at: utc_now(),
            },
        )
    };

    Ok(worktree)
}
```

Note: this preserves the existing tab-creation semantic (`t` always adds a window to the *current* session, even if the branch also has its own session). The spec discusses routing to the existing session-for-branch, but that's a session-list-level decision; at the `add_tab` API layer, we keep the behavior local. If you later want `t` on branch X to *switch* to X's session instead of attaching a window to the current one, that's a separate change in the inline-`t` handler in `session_list.rs`, not in `add_tab`.

- [ ] **Step 2: Verify build + tests**

Run: `cargo build && cargo test --lib`
Expected: clean.

- [ ] **Step 3: Smoke-test manually**

```bash
cargo install --path .
# In tmux + WezTerm:
# Alt+s → t → enter a branch name you already have a worktree for.
# Expected: window opens against the existing worktree, no error.
```

- [ ] **Step 4: Commit**

```bash
git add src/manager.rs
git commit -m "refactor: route add_tab through find_or_create_worktree"
```

---

## Task B6: Refactor `checkout_and_review` onto helpers

The PR-review (`R`) call path. Already has full reuse logic; this task reduces it to the same helpers for consistency.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Rewrite `checkout_and_review`**

Replace the body of `checkout_and_review` (currently `src/manager.rs:293-364`) with:

```rust
pub fn checkout_and_review(
    &self,
    repo_path: &str,
    pr_or_branch: &str,
) -> Result<(Session, String), String> {
    let repo = self.get_or_create_repo(repo_path);

    let branch = if pr_or_branch.chars().all(|c| c.is_ascii_digit()) && !pr_or_branch.is_empty() {
        let pr_num: i64 = pr_or_branch.parse().unwrap();
        git::get_pr_branch(repo_path, pr_num).map_err(|e| format!("Failed to get PR branch: {}", e))?
    } else {
        pr_or_branch.to_string()
    };

    git::fetch_branch(repo_path, &branch);

    // PR review roots the new local branch at the PR's HEAD on the remote,
    // not main — this preserves the original `checkout_and_review` semantic.
    let start_point = format!("origin/{}", branch);
    let wt_path = self.find_or_create_worktree(&repo, &branch, &start_point)?;
    let session = self.find_or_create_session(&repo, &branch, &wt_path)?;
    Ok((session, wt_path))
}
```

This replaces ~70 lines with ~12. The PR-number → branch resolution is preserved as the only special prefix.

- [ ] **Step 2: Verify build + tests**

Run: `cargo build && cargo test --lib`
Expected: clean.

- [ ] **Step 3: Smoke-test manually**

```bash
cargo install --path .
# In tmux + WezTerm:
# Alt+s → R → enter a PR number or branch name.
# Expected: same behavior as before (this is a refactor, not a feature change).
```

- [ ] **Step 4: Commit**

```bash
git add src/manager.rs
git commit -m "refactor: route checkout_and_review through find_or_create_* helpers"
```

---

# Wrap-up

## Task W1: Lint and final checks

- [ ] **Step 1: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings` (or without `-D warnings` if the project tolerates them; check existing CI config if any).
Expected: clean.

- [ ] **Step 2: Run the full test suite once more**

Run: `cargo test`
Expected: all green.

- [ ] **Step 3: Manual end-to-end smoke**

```bash
cargo install --path .
# Walk through: new session on existing branch (n), new tab on existing branch (t),
# PR review on a branch that already has a session (R), and use `/` to filter
# in the new-session repo picker, branch picker, history picker, edit-branch picker.
# Each filter should rank consecutive matches above gappy matches.
# Each worktree-creating flow should route to existing state rather than erroring.
```

- [ ] **Step 4: PR review base regression check**

This catches the bug the previous plan iteration had where `checkout_and_review` lost its `origin/<branch>` base. After running `R` on a real PR:

```bash
cd <the worktree trellis just created>
git rev-parse HEAD                  # PR's HEAD commit
git rev-parse origin/<branch>       # should match the above
```

Expected: the two SHAs match. If they don't, the worktree was branched off the wrong start point.

- [ ] **Step 5: Open a PR (or fast-forward main, your call)**

If you want to keep the upstream merge option open, push the branch to a personal GitHub fork and open a PR against `tomplex/trellis` (the upstream). If this is purely for your own use, fast-forward `main` to the branch tip locally and reinstall.

---

# Out of scope for this plan

- Updating `src/tui/session_list.rs` to add alphabetic tiebreak on score collisions. The spec's ranking contract technically applies, but session_list.rs's existing behavior (score-only sort) hasn't been a problem and the spec's "out of scope" implicitly covers it ("session_list.rs already uses fuzzy"). Revisit if score collisions feel weird in practice.
- Extracting a shared `filter_with_fuzzy<T>` helper. Picked option A1 (inline per-screen) during brainstorming.
- Switching to the `nucleo` crate.
- Cross-repo worktree reuse or "force recreate" affordances.
