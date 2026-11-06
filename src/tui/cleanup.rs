// trellis/src/tui/cleanup.rs — worktree cleanup screen

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::git;
use crate::manager::Manager;
use crate::utils::truncate_start;

use super::confirm::ConfirmScreen;
use super::job::{Job, JobMsg};
use super::theme;
use super::{ActionResult, ScreenAction, ScreenBehavior};

#[derive(Clone)]
enum RowSource {
    /// Worktree Trellis tracks in its DB.
    Tracked { wt_id: i64 },
    /// Worktree git knows about but Trellis doesn't (created out-of-band, or
    /// its DB row was lost).
    Orphan,
}

/// Per-row state during a bulk delete. Rows still in the queue carry `Queued`
/// until the worker reaches them; the in-flight row is derived from the job's
/// completed count, since the worker drains its queue in order.
enum RowDelete {
    Queued,
    Deleted,
    Failed(String),
}

struct WorktreeRow {
    source: RowSource,
    branch: String,
    session_name: String,
    path: String,
    /// The owning repo's working tree, used both for `git worktree remove` on
    /// orphans and for the staleness check.
    repo_path: String,
    repo_default_branch: String,
    stale: Option<bool>, // None = still checking
    delete: Option<RowDelete>,
}

/// One removal, resolved on the main thread before the worker starts so the
/// sqlite connection never crosses the thread boundary.
struct DeleteTask {
    path: String,
    branch: String,
    repo_path: String,
}

/// Result of one removal, for the main thread to commit.
struct DeleteOutcome {
    path: String,
    error: Option<String>,
}

/// Inputs the staleness worker needs per row. Lives across the thread
/// boundary, so it owns its strings.
struct StaleCheckInput {
    path: String,
    repo_path: String,
    branch: String,
    default_branch: String,
}

pub struct CleanupScreen {
    rows: Vec<WorktreeRow>,
    /// Selected rows, keyed by worktree path. Path is unique across both
    /// tracked and orphan rows, so it works as a stable identity for either.
    selected: HashSet<String>,
    table_state: TableState,
    error_message: Option<String>,
    stale_rx: Option<mpsc::Receiver<HashSet<String>>>,
    delete_job: Option<Job<DeleteOutcome>>,
    /// Worktree paths in worker order; indexed by the job's completed count to
    /// find the row currently being removed.
    delete_queue: Vec<String>,
}

impl CleanupScreen {
    pub fn new(manager: &Manager) -> Self {
        let tracked = manager.get_all_worktrees();
        let orphans = manager.list_orphan_worktrees();
        let sessions = manager.get_sessions();
        let repos = manager.get_repos();

        let session_map: HashMap<i64, String> = sessions
            .into_iter()
            .filter_map(|s| s.id.map(|id| (id, s.name.clone())))
            .collect();
        let repo_map: HashMap<i64, crate::models::Repo> = repos
            .into_iter()
            .filter_map(|r| r.id.map(|id| (id, r)))
            .collect();

        let mut rows: Vec<WorktreeRow> = Vec::new();

        // DB-tracked worktrees
        for wt in &tracked {
            let Some(wt_id) = wt.id else { continue };
            let Some(repo) = repo_map.get(&wt.repo_id) else { continue };
            let session_name = wt
                .session_id
                .and_then(|sid| session_map.get(&sid).cloned())
                .unwrap_or_else(|| "Unattached".to_string());
            rows.push(WorktreeRow {
                source: RowSource::Tracked { wt_id },
                branch: wt.branch.clone(),
                session_name,
                path: wt.path.clone(),
                repo_path: repo.path.clone(),
                repo_default_branch: repo.default_branch.clone(),
                stale: None,
                delete: None,
            });
        }

        // Worktrees git knows about but Trellis doesn't.
        // `list_orphan_worktrees` already excludes the main worktree, missing
        // dirs, detached HEAD, and paths Trellis already tracks.
        for orphan in &orphans {
            rows.push(WorktreeRow {
                source: RowSource::Orphan,
                branch: orphan.branch.clone(),
                session_name: "(orphan)".to_string(),
                path: orphan.path.clone(),
                repo_path: orphan.repo.path.clone(),
                repo_default_branch: orphan.repo.default_branch.clone(),
                stale: None,
                delete: None,
            });
        }

        rows.sort_by(|a, b| {
            a.session_name
                .to_lowercase()
                .cmp(&b.session_name.to_lowercase())
                .then_with(|| a.branch.to_lowercase().cmp(&b.branch.to_lowercase()))
        });

        let mut table_state = TableState::default();
        if !rows.is_empty() {
            table_state.select(Some(0));
        }

        // Spawn background thread for staleness check. Hand it owned data
        // rather than database handles so it can outlive `manager`.
        let inputs: Vec<StaleCheckInput> = rows
            .iter()
            .map(|r| StaleCheckInput {
                path: r.path.clone(),
                repo_path: r.repo_path.clone(),
                branch: r.branch.clone(),
                default_branch: r.repo_default_branch.clone(),
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stale_paths = HashSet::new();
            for input in &inputs {
                let merged = git::is_branch_merged(
                    &input.repo_path,
                    &input.branch,
                    &input.default_branch,
                )
                .unwrap_or(false);
                let has_remote = git::has_remote_branch(&input.repo_path, &input.branch)
                    .unwrap_or(true);
                if merged || !has_remote {
                    stale_paths.insert(input.path.clone());
                }
            }
            let _ = tx.send(stale_paths);
        });

        Self {
            rows,
            selected: HashSet::new(),
            table_state,
            error_message: None,
            stale_rx: Some(rx),
            delete_job: None,
            delete_queue: Vec::new(),
        }
    }

    fn deleting(&self) -> bool {
        self.delete_job.is_some()
    }

    /// The row the worker is on right now — it drains its queue in order, so the
    /// completed count is the index of the in-flight task.
    fn active_delete_path(&self) -> Option<&str> {
        let job = self.delete_job.as_ref()?;
        self.delete_queue.get(job.done()).map(String::as_str)
    }

    fn check_stale_results(&mut self) {
        if let Some(ref rx) = self.stale_rx {
            if let Ok(stale_paths) = rx.try_recv() {
                for row in &mut self.rows {
                    row.stale = Some(stale_paths.contains(&row.path));
                }
                self.stale_rx = None;
            }
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let max = self.rows.len() as i32 - 1;
        let next = (current + delta).clamp(0, max) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle_current(&mut self) {
        if let Some(idx) = self.table_state.selected() {
            if idx < self.rows.len() {
                let path = self.rows[idx].path.clone();
                if !self.selected.remove(&path) {
                    self.selected.insert(path);
                }
            }
        }
    }

    fn select_all(&mut self) {
        for row in &self.rows {
            self.selected.insert(row.path.clone());
        }
    }

    fn deselect_all(&mut self) {
        self.selected.clear();
    }

    fn stale_count(&self) -> usize {
        self.rows.iter().filter(|r| r.stale == Some(true)).count()
    }

    fn status_line(&self) -> Line<'_> {
        let total = self.rows.len();
        let stale = self.stale_count();
        let sel = self.selected.len();

        Line::from(vec![
            Span::styled(
                format!(" {} worktrees", total),
                Style::default().fg(theme::TEXT_DIM),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} stale", stale),
                Style::default().fg(theme::YELLOW),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} selected", sel),
                Style::default().fg(theme::ACCENT),
            ),
        ])
    }

    /// Hand every selected worktree's `git worktree remove` to a worker. Removing
    /// a working copy takes seconds, so doing this inline froze the whole TUI for
    /// the length of the batch.
    fn start_delete(&mut self) {
        // Worker order follows display order so the progress line reads top-down.
        let tasks: Vec<DeleteTask> = self
            .rows
            .iter()
            .filter(|r| self.selected.contains(&r.path))
            .map(|r| DeleteTask {
                path: r.path.clone(),
                branch: r.branch.clone(),
                repo_path: r.repo_path.clone(),
            })
            .collect();

        if tasks.is_empty() {
            return;
        }

        self.error_message = None;
        self.delete_queue = tasks.iter().map(|t| t.path.clone()).collect();
        for row in &mut self.rows {
            if self.selected.contains(&row.path) {
                row.delete = Some(RowDelete::Queued);
            }
        }

        let total = tasks.len();
        self.delete_job = Some(Job::spawn("Deleting", Some(total), move |tx| {
            for task in tasks {
                if tx.send(JobMsg::Status(task.branch)).is_err() {
                    return; // screen went away
                }
                let error = match git::remove_worktree(&task.repo_path, &task.path) {
                    Ok(()) => None,
                    // Directory already gone — the DB record should still go.
                    Err(_) if !std::path::Path::new(&task.path).exists() => None,
                    Err(e) => Some(e.to_string()),
                };
                let outcome = DeleteOutcome { path: task.path, error };
                if tx.send(JobMsg::Item(outcome)).is_err() {
                    return;
                }
            }
            let _ = tx.send(JobMsg::Done);
        }));
    }

    /// Commit finished removals. The worker only touches git; the DB delete has
    /// to happen here because the connection can't cross a thread boundary.
    /// Orphan rows have no DB record to forget.
    fn apply_delete_results(&mut self, manager: &Manager) {
        let outcomes = match self.delete_job.as_mut() {
            Some(job) => job.drain(),
            None => return,
        };

        for outcome in outcomes {
            let Some(row) = self.rows.iter_mut().find(|r| r.path == outcome.path) else {
                continue;
            };
            row.delete = Some(match outcome.error {
                None => {
                    if let RowSource::Tracked { wt_id } = row.source {
                        manager.forget_worktree(wt_id);
                    }
                    self.selected.remove(&outcome.path);
                    RowDelete::Deleted
                }
                Some(msg) => RowDelete::Failed(msg),
            });
        }

        if self.delete_job.as_ref().is_some_and(|j| j.finished()) {
            self.finish_delete();
        }
    }

    /// Drop the removed rows and surface any failures. Failed rows stay put and
    /// stay selected so `d` retries them.
    fn finish_delete(&mut self) {
        let failures: Vec<String> = self
            .rows
            .iter()
            .filter_map(|r| match &r.delete {
                Some(RowDelete::Failed(msg)) => Some(format!("{}: {}", r.path, msg)),
                _ => None,
            })
            .collect();

        self.rows
            .retain(|r| !matches!(r.delete, Some(RowDelete::Deleted)));
        self.error_message = (!failures.is_empty()).then(|| failures.join("; "));
        self.delete_job = None;
        self.delete_queue.clear();

        // Fix cursor if it's past the end
        if self.rows.is_empty() {
            self.table_state.select(None);
        } else {
            let max = self.rows.len() - 1;
            if self.table_state.selected().unwrap_or(0) > max {
                self.table_state.select(Some(max));
            }
        }
    }

    fn render_footer(&self) -> Paragraph<'_> {
        if self.deleting() {
            return super::rename::render_footer_bindings(&[
                ("j/k", "navigate"),
                ("…", "delete in progress"),
            ]);
        }
        super::rename::render_footer_bindings(&[
            ("esc", "back"),
            ("j/k", "navigate"),
            ("space", "toggle"),
            ("a", "select all"),
            ("A", "deselect all"),
            ("d", "delete"),
        ])
    }
}

impl ScreenBehavior for CleanupScreen {
    fn tick(&mut self, manager: &mut Manager) -> ScreenAction {
        self.check_stale_results();
        self.apply_delete_results(manager);
        ScreenAction::None
    }

    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let job_height = if self.deleting() { 1 } else { 0 };
        let error_height = if self.error_message.is_some() { 1 } else { 0 };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // status bar
                Constraint::Length(job_height), // background work indicator
                Constraint::Length(error_height), // error
                Constraint::Min(0),    // table
                Constraint::Length(1), // footer
            ])
            .split(area);

        // Title
        let title = Paragraph::new(Line::from(vec![Span::styled(
            " Cleanup Worktrees",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(title, chunks[0]);

        // Status bar
        let status = Paragraph::new(self.status_line()).style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(status, chunks[1]);

        // Background work indicator
        if let Some(ref job) = self.delete_job {
            let indicator = Paragraph::new(job.indicator_line())
                .style(Style::default().bg(theme::HEADER_BG));
            f.render_widget(indicator, chunks[2]);
        }

        // Error
        if let Some(ref err) = self.error_message {
            let err_widget =
                Paragraph::new(format!(" Error: {}", err)).style(Style::default().fg(theme::RED));
            f.render_widget(err_widget, chunks[3]);
        }

        // Table
        let header = Row::new(vec![
            Cell::from(" "),
            Cell::from("Branch"),
            Cell::from("Session"),
            Cell::from("Path"),
            Cell::from("Status"),
        ])
        .style(theme::style_header())
        .height(1);

        let active_delete = self.active_delete_path();
        let table_rows: Vec<Row> = self
            .rows
            .iter()
            .map(|row| {
                let is_selected = self.selected.contains(&row.path);

                let checkbox = if is_selected {
                    Span::styled("[x]", Style::default().fg(theme::BLUE))
                } else {
                    Span::styled("[ ]", Style::default().fg(theme::TEXT_DIM))
                };

                let branch_style = if row.stale == Some(true) {
                    Style::default().fg(theme::YELLOW)
                } else {
                    Style::default().fg(theme::TEXT)
                };

                // Delete state outranks staleness — it's the live information.
                let status_span = match &row.delete {
                    Some(RowDelete::Deleted) => {
                        Span::styled("deleted", Style::default().fg(theme::GREEN))
                    }
                    Some(RowDelete::Failed(_)) => {
                        Span::styled("failed", Style::default().fg(theme::RED))
                    }
                    Some(RowDelete::Queued) if active_delete == Some(row.path.as_str()) => {
                        Span::styled("deleting…", Style::default().fg(theme::ACCENT))
                    }
                    Some(RowDelete::Queued) => {
                        Span::styled("queued", Style::default().fg(theme::TEXT_DIM))
                    }
                    None => match row.stale {
                        None => Span::styled("checking…", Style::default().fg(theme::TEXT_DIM)),
                        Some(true) => Span::styled("stale", Style::default().fg(theme::YELLOW)),
                        Some(false) => Span::styled("ok", Style::default().fg(theme::GREEN)),
                    },
                };

                Row::new(vec![
                    Cell::from(checkbox),
                    Cell::from(Span::styled(&*row.branch, branch_style)),
                    Cell::from(Span::styled(&*row.session_name, Style::default().fg(theme::TEXT))),
                    Cell::from(Span::styled(
                        truncate_start(&row.path, 40),
                        Style::default().fg(theme::TEXT_DIM),
                    )),
                    Cell::from(status_span),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(3),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Length(10),
        ];

        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(theme::style_cursor())
            .style(theme::style_default())
            .block(Block::default().style(theme::style_default()));

        let mut state = self.table_state.clone();
        f.render_stateful_widget(table, chunks[4], &mut state);

        // Footer
        let footer = self.render_footer();
        f.render_widget(footer, chunks[5]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        // Always check for background results (even on non-key events)
        self.check_stale_results();
        self.apply_delete_results(manager);

        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return ScreenAction::None;
        };

        // Navigation stays live mid-delete, but anything that mutates the list or
        // leaves the screen is blocked: popping drops the receiver, which would
        // strand the worker's results and leave the DB claiming live worktrees.
        if self.deleting() {
            return match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_cursor(1);
                    ScreenAction::None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_cursor(-1);
                    ScreenAction::None
                }
                _ => ScreenAction::None,
            };
        }

        match code {
            KeyCode::Esc => ScreenAction::Pop,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor(1);
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor(-1);
                ScreenAction::None
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.toggle_current();
                ScreenAction::None
            }
            KeyCode::Char('A') => {
                // Shift+A: deselect all
                self.deselect_all();
                ScreenAction::None
            }
            KeyCode::Char('a') => {
                self.select_all();
                ScreenAction::None
            }
            KeyCode::Char('d') => {
                if self.selected.is_empty() {
                    return ScreenAction::None;
                }
                let count = self.selected.len();
                ScreenAction::Push(Box::new(ConfirmScreen::new(
                    "Confirm Deletion".to_string(),
                    format!("Delete {} worktree(s)?", count),
                )))
            }
            _ => ScreenAction::None,
        }
    }

    fn on_child_result(&mut self, result: ActionResult, _manager: &mut Manager) -> ScreenAction {
        if let ActionResult::Confirmed(true) = result {
            self.start_delete();
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::{Repo, Worktree};
    use crate::utils::utc_now;
    use rusqlite::Connection;
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;

    /// A real repo with `count` real worktrees, registered in an in-memory DB.
    fn repo_with_worktrees(root: &Path, count: usize) -> (Manager, Vec<String>) {
        // git reports canonical paths; a symlinked tempdir (macOS /var) would make
        // every tracked worktree also surface as an orphan.
        let root = &root.canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            Command::new("git").args(args).current_dir(&repo).output().unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "--allow-empty", "-qm", "init"]);
        git(&["branch", "-M", "main"]);

        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn);
        let repo_row = db::add_repo(
            &conn,
            &Repo {
                id: None,
                path: repo.to_string_lossy().to_string(),
                name: "repo".to_string(),
                default_branch: "main".to_string(),
            },
        );

        let mut paths = Vec::new();
        for i in 0..count {
            let branch = format!("feat/wt-{}", i);
            let wt_path = root
                .join(format!("worktrees/wt-{}", i))
                .to_string_lossy()
                .to_string();
            Command::new("git")
                .args(["worktree", "add", "-q", "-b", &branch, &wt_path, "main"])
                .current_dir(&repo)
                .output()
                .unwrap();
            db::add_worktree(
                &conn,
                &Worktree {
                    id: None,
                    repo_id: repo_row.id.unwrap(),
                    path: wt_path.clone(),
                    branch,
                    session_id: None,
                    tmux_window: None,
                    created_at: utc_now(),
                },
            );
            paths.push(wt_path);
        }
        (Manager::new(conn), paths)
    }

    fn run_to_completion(screen: &mut CleanupScreen, manager: &Manager) {
        while screen.deleting() {
            screen.apply_delete_results(manager);
        }
    }

    #[test]
    fn start_delete_hands_off_without_committing_anything() {
        let tmp = tempdir().unwrap();
        let (manager, paths) = repo_with_worktrees(tmp.path(), 4);
        let mut screen = CleanupScreen::new(&manager);
        screen.select_all();

        screen.start_delete();

        // The whole point of the change: control returns to the event loop with
        // the removals still outstanding. Doing the work inline here — as the
        // old perform_delete did — would have committed before returning.
        assert!(screen.deleting(), "a job should be in flight");
        assert_eq!(
            manager.get_all_worktrees().len(),
            paths.len(),
            "start_delete must not touch the DB; only the worker's results may"
        );
    }

    #[test]
    fn draining_the_job_clears_disk_and_db() {
        let tmp = tempdir().unwrap();
        let (manager, paths) = repo_with_worktrees(tmp.path(), 4);
        let mut screen = CleanupScreen::new(&manager);
        screen.select_all();

        screen.start_delete();
        run_to_completion(&mut screen, &manager);

        assert!(
            paths.iter().all(|p| !Path::new(p).exists()),
            "every worktree directory removed"
        );
        assert!(
            manager.get_all_worktrees().is_empty(),
            "every DB record removed"
        );
        assert!(screen.rows.is_empty(), "deleted rows leave the table");
        assert!(screen.selected.is_empty(), "deleted rows leave the selection");
        assert!(screen.error_message.is_none());
    }

    #[test]
    fn worktree_whose_directory_vanished_is_still_forgotten() {
        let tmp = tempdir().unwrap();
        let (manager, paths) = repo_with_worktrees(tmp.path(), 2);
        // Nuke one behind git's back — `git worktree remove` will fail on it.
        std::fs::remove_dir_all(&paths[0]).unwrap();

        let mut screen = CleanupScreen::new(&manager);
        screen.select_all();
        screen.start_delete();
        run_to_completion(&mut screen, &manager);

        assert!(
            manager.get_all_worktrees().is_empty(),
            "a vanished directory must not strand its DB row"
        );
        assert!(screen.error_message.is_none());
    }

    #[test]
    fn unselected_worktrees_are_left_alone() {
        let tmp = tempdir().unwrap();
        let (manager, paths) = repo_with_worktrees(tmp.path(), 3);
        let mut screen = CleanupScreen::new(&manager);
        let kept = screen.rows[0].path.clone();
        for row in &screen.rows[1..] {
            screen.selected.insert(row.path.clone());
        }

        screen.start_delete();
        run_to_completion(&mut screen, &manager);

        let left = manager.get_all_worktrees();
        assert_eq!(left.len(), 1, "only the selected rows are removed");
        assert_eq!(left[0].path, kept);
        assert!(
            paths.iter().filter(|p| Path::new(p).exists()).count() == 1,
            "the unselected worktree stays on disk"
        );
    }
}
