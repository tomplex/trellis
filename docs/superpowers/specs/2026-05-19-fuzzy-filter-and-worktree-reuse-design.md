# Fuzzy Filter and Worktree Reuse

Two related quality-of-life changes to trellis:

1. **Fuzzy filtering** in every picker screen. The session list already uses the in-tree `fuzzy_match` scorer; the other picker screens use substring matching, which feels worse once you've used the session list.
2. **Worktree reuse** on `n` (new session) and `t` (new tab). Today these flows can hard-fail with "Worktree at '...' already exists" when a worktree for the chosen branch is already on disk. They should instead route the user to where their work already lives, matching what `R` (PR review) already does.

## Motivation

The fuzzy and worktree-reuse changes are independent but both fix the same kind of friction: trellis sometimes punishes the operator for not knowing internal state (substring filtering surprises you with no results; "worktree exists" errors force you to manually delete things before you can retry). Both changes lean trellis toward "do the obvious thing" instead.

---

## Part A — Fuzzy Filtering Across Pickers

### Current state

`src/fuzzy.rs` defines `fuzzy_match(query, text) -> Option<i32>` — a case-insensitive in-order matcher that scores consecutive runs lower (better) and gaps by position. `src/tui/session_list.rs` uses it.

Four other screens implement filtering with `text.to_lowercase().contains(&query)`:

- `src/tui/new_session.rs::apply_filter` — repos, branches, subsystems
- `src/tui/edit_branch.rs::rebuild_display` — branches
- `src/tui/adopt_session.rs::apply_filter` — repos, branches
- `src/tui/history.rs` — multi-field across `date | project | branch | summary`

### Change

Replace `.to_lowercase().contains()` with `fuzzy_match(&query, candidate)` in each of the four screens. Sort matches by score (lower is better); break ties alphabetically so ordering is stable across keystrokes.

`history.rs` filters across four fields per row. Take the **best (lowest)** score across the four fields per row, and use that for ranking. Break score ties on `(project, branch, date)` lowercased — `project` is the natural anchor a user scanning history will expect.

### Ranking contract

| Property | Behavior |
|---|---|
| Score source | `fuzzy_match` (unchanged) |
| Sort key | Ascending score, then ascending lowercased label |
| Empty query | Show all rows in their natural (existing) order |
| No matches | Show empty list (existing behavior preserved) |

### Out of scope (deliberately)

- Extracting a shared `filter_with_fuzzy<T>` helper. The five callers do slightly different things (single-field vs multi-field, with/without "add new entry" affordance); the abstraction starts paying for itself only beyond this point. Inline the swap and keep the call sites readable.
- Switching to a third-party fuzzy crate (e.g. `nucleo`). Defer until the in-tree scorer feels insufficient in real use.

---

## Part B — Worktree Reuse on `n` and `t`

### Current state

Three call sites in `src/manager.rs` create worktrees, each with different reuse behavior:

| Call site | Triggered by | Reuse logic today |
|---|---|---|
| `checkout_and_review` (line 293) | `R` | Full reuse: DB session lookup + dir-exists check, returns existing session+path |
| `create_session` (line 159) | `n` | Partial: on `git worktree add` failure, falls through if `wt_path` exists on disk; does not consult the DB |
| `add_tab` (line 367) | `t` | None: hard-fails on existing worktree with "Failed to create worktree: ..." |

`git::create_worktree` (`src/git.rs:128`) pre-checks by iterating `list_worktrees()` and erroring if the *exact path* is already registered. This is the surface the failures bubble up from.

### Change

Add two private helpers to `Manager`:

```rust
// Resolve the on-disk worktree for (repo, branch). If git already has one
// registered for this branch, return its recorded path. If the dir was
// deleted manually (prunable), prune and recreate. Otherwise create fresh.
fn find_or_create_worktree(&self, repo: &Repo, branch: &str) -> Result<String, String>;

// Resolve the trellis session for (repo, branch). If a DB row exists AND the
// underlying tmux session is alive, return it. If the DB row is stale, drop
// it. Otherwise add the DB row, create the tmux session/window, apply layout.
fn find_or_create_session(
    &self,
    repo: &Repo,
    branch: &str,
    wt_path: &str,
) -> Result<Session, String>;
```

Refactor the three call sites onto these helpers. `checkout_and_review` keeps its PR-number-to-branch resolution prefix; everything after that reduces to the two helpers. `create_session` and `add_tab` collapse to: resolve worktree → resolve session → return.

### Detection contract

| Question | Source of truth |
|---|---|
| Does a worktree exist for this branch? | `git worktree list --porcelain` (via `git::list_worktrees`). Matched by `branch` field, not by path. |
| Where does that worktree live on disk? | Git's recorded path — *not* `Manager::worktree_path(repo, branch)`. A user who moved a worktree should still get found. |
| Does a trellis session exist for this branch? | DB lookup by `(repo_id, branch)`. |
| Is that session still real? | `tmux::session_exists(name)` (defined in `src/tmux.rs`). If the DB row points at a dead tmux session, the DB row is stale. |

### Reuse semantics

- **`n` (new session) on branch X, when X already has a session:** route to the existing session. Don't create a duplicate. The user lands where their work already lives.
- **`t` (new tab in session S) on branch X, when X already has a session S':** route to S' (consistent with "go where it lives"). This is an unusual workflow — the common case is `t` on a fresh branch — but the behavior should be predictable.
- **Worktree exists but no session exists yet:** create the session pointing at the existing worktree path (no `git worktree add` call).
- **Worktree dir was deleted manually (git considers it prunable):** run `git worktree prune` via a new `git::prune_worktrees(repo_path)` helper in `src/git.rs`, then create fresh at the trellis-convention path.

### Edge cases

| Case | Behavior |
|---|---|
| Worktree dir deleted manually (prunable) | Prune, then create fresh at `Manager::worktree_path(...)` |
| Worktree exists with uncommitted changes | No special handling — reuse normally; user sees their changes |
| DB session row exists, tmux session is dead | Drop the stale DB row first, then insert the fresh session row — in that order, so any unique index on `(repo_id, branch)` or `name` doesn't trip on the replace |
| User picks the repo's default branch | Same as today — no worktree created, session points at repo root |
| Two repos contain the same branch name | Scoped by `repo_id`, no collision |
| Branch exists in git but not as a worktree | Existing behavior: create worktree from that branch |

### Out of scope (deliberately)

- "Force recreate" affordance. If the user genuinely wants a fresh worktree, they delete the existing tab/session first (`d`).
- Cross-repo worktree reuse. A worktree for `feat/x` in repo A and a worktree for `feat/x` in repo B are independent.
- Caching `git worktree list` results on the `Manager`. The list changes outside trellis (manual `git worktree add`, other tools); caching adds invalidation cost for marginal speed gain.

---

## Testing

trellis uses inline `#[cfg(test)]` modules; no `tests/` directory. Follow that pattern.

**Fuzzy:** `fuzzy_match` itself is already well-covered in `src/fuzzy.rs`. Per picker, add one small test that confirms filtering narrows the list and sorts by score. Avoid duplicating the matcher's own test cases.

**Worktree reuse:** real risk lives here. Add tests in `src/manager.rs` (using a temp git repo via `tempfile` if not already used, otherwise a small helper) for `find_or_create_worktree` and `find_or_create_session`:

- create fresh when nothing exists
- reuse when worktree+session exist
- reuse worktree, create session, when session is missing
- recover from prunable worktree
- drop stale DB row when tmux session is dead
- default-branch path (no worktree created)

The three call sites (`create_session`, `add_tab`, `checkout_and_review`) become thin enough that their existing behavioral coverage (if any) plus the helper tests should suffice.

---

## Acceptance

- All five picker screens (session list, new session, edit branch, adopt session, history) rank results via `fuzzy_match` with the contract above.
- `t` on a branch with an existing worktree no longer errors; it routes to the existing session.
- `n` on a branch with an existing session no longer creates a duplicate; it routes to the existing session.
- `R` continues to work exactly as it does today (refactored onto the same helpers, no behavior change).
- New helpers covered by inline tests.
