# Background work indication

## Problem

Deleting 14 worktrees from the cleanup screen looks like the app has hung. It has not
crashed — it is doing exactly the work that was asked for — but nothing on screen says so.

The cause is structural. The event loop in `src/tui/mod.rs` is single-threaded:

```rust
while !self.should_quit {
    terminal.draw(|f| self.render(f));          // draw
    top.tick(&mut self.manager);                // tick
    if event::poll(Duration::from_millis(50)) { // then, and only then, read input
        …
    }
}
```

Any slow work performed inside `handle_event`, `on_child_result`, `on_resume`, or a
screen constructor runs *between* draws. For its whole duration there is no repaint and
no input polling. `CleanupScreen::perform_delete` (`src/tui/cleanup.rs:186`) loops over
every selected worktree calling `git worktree remove --force`, which deletes a full
working copy from disk. Fourteen of those is tens of seconds of a dead terminal.

An audit found twelve more sites with the same shape, ranked in the plan document. The
two that matter most after bulk delete are the session-list refresh — which spawns two
`tmux` processes *per typed character* in the filter, plus one `capture-pane` per
expanded Claude window — and the network-bound `gh pr view` / `git fetch` paths.

## Constraint that shapes the solution

`Manager` owns a `rusqlite::Connection` (`src/manager.rs:129`). `Connection` is `Send`
but not `Sync`, and `src/db.rs:42` sets only `PRAGMA foreign_keys` — there is no WAL and
no `busy_timeout`, so the database is in default rollback-journal mode. Opening a second
write connection on a worker thread would risk `SQLITE_BUSY`.

**Therefore: no database work moves off the main thread.** Workers perform shell-outs and
filesystem walks only, and send plain owned data back. The main thread commits.

This costs nothing. A `db::delete_worktree` is microseconds; the `git worktree remove`
next to it is seconds. Only the slow half needs to move. It also matches the one
background pattern the codebase already has — `CleanupScreen::new` pulls owned
`Vec<Worktree>` and `Vec<Repo>` off the manager *before* `thread::spawn`, and never lets
the connection cross the boundary.

A useful related finding: all eighteen `Manager` methods take `&self`, not `&mut self`.
The `&mut Manager` in the `ScreenBehavior` signature is vestigial and does not obstruct
anything here.

## Architecture

One new module, `src/job.rs`. No change to the `ScreenBehavior` trait and no change to
the event loop — the loop already redraws at roughly 20 Hz when idle, because
`event::poll(50ms)` returns after its timeout and falls through to the next `draw`. That
is enough to animate a spinner for free.

```rust
pub enum JobMsg<T> {
    Item(T),          // one unit finished; payload is what the main thread must commit
    Status(String),   // advisory label change, for indeterminate work
    Done,
}

pub struct Job<T> {
    rx: mpsc::Receiver<JobMsg<T>>,
    label: String,
    done: usize,
    total: Option<usize>,     // None = indeterminate, render a bare spinner
    current: Option<String>,
    finished: bool,
}
```

Three operations:

- `Job::spawn(label, total, move |tx| …)` — spawns the worker, hands it the sender.
- `Job::drain() -> Vec<T>` — non-blocking `try_recv` loop. Advances the counters and
  returns the payloads for the caller to apply. Called from `tick()`.
- `Job::indicator_line() -> Line` — the shared spinner renderer, so a job looks the same
  on every screen: `⠹ Deleting 6/14 — feat/fig-v2-attrs`.

A screen holds `Option<Job<T>>`; `Some` *is* the "busy" flag, mirroring how
`stale_rx: Option<Receiver<…>>` already works on the cleanup screen.

### Known limitation

`tick()` is only called on `screen_stack.last_mut()`. A screen that starts a job and then
pushes a modal on top of itself will not drain that job until the modal pops. No planned
phase does this — the bulk delete starts *after* its confirm modal is dismissed — but the
constraint should be respected by future callers, and revisited if a phase needs it.

## Phases

Each phase is a separate commit so the behavioural ones can be reviewed or dropped on
their own.

### Phase 1 — bulk worktree delete (purely additive)

The reported bug. `git::remove_worktree` moves to a worker that emits one `Item` per
worktree; the main thread applies `db::delete_worktree` as each result lands.
`WorktreeRow` gains a per-row delete state rendered in the existing Status column
(`deleting…` → `deleted`, or the error), alongside the progress line. Failures no longer
abort the batch and no longer collapse into one joined error string — each row carries
its own.

### Phase 2 — session-list refresh (changes behaviour)

Filtering becomes a pure in-memory operation over already-loaded rows; keystrokes stop
triggering process spawns. A background job re-polls tmux on an interval of roughly one
second and swaps in a fresh snapshot, so the list stays near-live without blocking input.
The consequence, accepted deliberately: the list may lag reality by up to that interval.

### Phase 3 — network-bound one-shots (changes behaviour)

`create_session`, `checkout_and_review`, `resurrect_session`, and `add_tab`. These are the
`gh pr view` and `git fetch` paths — two network round trips in the worst case. Each
currently ends in `ScreenAction::Switch`, which writes a switch file and quits the app, so
the work cannot simply be fired and forgotten: the switch has to wait for the job. Each
gets a progress modal that holds the switch until completion and surfaces failures in
place, rather than after the fact via `status_message`.

### Phase 4 — cheap pure wins (purely additive)

The conversation-index parse in `HistoryScreen::new`, `git::list_branches` in both wizards
and the edit-branch modal, and the repos-directory walk in `NewSessionScreen::new`. None
touches the database, so each is a drop-in worker plus a `loading…` state.

## Status

Landed: Phase 1, Phase 2, the conversation-index parse from Phase 4, and the PR
review fetch from Phase 3.

Not landed, still blocking on the event loop:

- **Phase 3 remainder.** `create_session` still calls `git::fetch_and_pull`
  (`manager.rs`) inline — the same network cost as the review path and the same
  clean seam, since the fetch precedes all db work. `resurrect_session` and
  `add_tab` are dominated by `git worktree add` and roughly eight tmux spawns;
  both are local, so they are less severe but still visible on a large repo.
- **Phase 4 remainder.** `wizard_load_branches` (`tui/mod.rs`, used by
  `new_session`, `adopt_session` and `edit_branch`) and the repos-directory walk
  in `NewSessionScreen::new`. Both are one-shots in the millisecond range on a
  normal repo, which is why they were left for last.

## Testing

`Job` is tested without threads by constructing it around a channel and pushing `JobMsg`
values directly, which keeps the tests deterministic — counter advancement, `drain`
returning payloads in order, and the transition to finished on `Done`.

Screen-level logic is tested the same way: the apply-a-result half of each conversion is a
plain function taking a payload, so it can be exercised without spawning anything. The
worker halves are thin wrappers over `git::`/`tmux::` functions that already have their own
coverage.

Phases 2 and 3 change observable behaviour and are verified by running the binary, not
only by unit tests.
