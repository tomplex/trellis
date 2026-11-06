// trellis/src/manager.rs
//! High-level orchestration: ties DB, tmux, and git together.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::db;
use crate::git;
use crate::git::GitWorktree;
use crate::models::{Repo, Session, SessionInfo, Worktree};
use crate::tmux;

use crate::utils::utc_now;

/// Window the agent runs in; second window is a plain shell.
pub const AGENT_WINDOW: &str = "agent";
const SHELL_WINDOW: &str = "shell";
/// Agent launched when `agent_cmd` is unset.
pub const DEFAULT_AGENT_CMD: &str = "claude";

/// A worktree git knows about that Trellis hasn't registered yet.
#[derive(Debug, Clone)]
pub struct OrphanWorktree {
    pub repo: Repo,
    pub branch: String,
    pub path: String,
}

/// Pure filter: given repos, the set of paths Trellis already tracks, and
/// per-repo `git worktree list` results, return the worktrees Trellis should
/// surface as adoptable. Excludes the repo's main path, anything in
/// `tracked_paths`, and detached-HEAD entries. The IO-level missing-directory
/// filter happens in `Manager::list_orphan_worktrees` before calling this.
fn filter_orphans(
    repos: &[Repo],
    tracked_paths: &HashSet<String>,
    git_results: &HashMap<i64, Vec<GitWorktree>>,
) -> Vec<OrphanWorktree> {
    let mut out: Vec<OrphanWorktree> = Vec::new();
    for repo in repos {
        let Some(repo_id) = repo.id else { continue };
        let Some(entries) = git_results.get(&repo_id) else { continue };
        for entry in entries {
            if entry.path == repo.path {
                continue; // main worktree
            }
            if tracked_paths.contains(&entry.path) {
                continue; // already tracked
            }
            let Some(branch) = entry.branch.as_ref() else {
                continue; // defensive: parser doesn't currently produce None
            };
            if branch == "(detached)" {
                continue;
            }
            out.push(OrphanWorktree {
                repo: repo.clone(),
                branch: branch.clone(),
                path: entry.path.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.repo.name.cmp(&b.repo.name).then_with(|| a.branch.cmp(&b.branch))
    });
    out
}

/// Create a tmux session and set up the default layout: an agent window
/// running `agent_cmd` and a shell window, both rooted at `working_dir`.
///
/// The agent window becomes the session's initial window — named at
/// `tmux new-session` time via `-n`, so we don't depend on tmux's
/// configurable `base-index`.
pub fn apply_layout(session_name: &str, working_dir: &str, agent_cmd: &str) -> Result<(), tmux::TmuxError> {
    tmux::new_session(session_name, working_dir, Some(AGENT_WINDOW))?;
    let target = format!("{}:{}", session_name, AGENT_WINDOW);
    tmux::send_keys(&target, &[agent_cmd, "Enter"]);

    let _ = tmux::new_window(session_name, SHELL_WINDOW, Some(working_dir));

    // Focus the agent window by name (base-index agnostic).
    let _ = tmux::select_window_by_name(session_name, AGENT_WINDOW);
    Ok(())
}

/// Find or create the on-disk worktree for (repo, branch).
///
/// Git's `worktree list` is the source of truth. If a worktree for `branch`
/// already exists, returns its recorded path (even if outside `worktrees_dir`).
/// If git considers a worktree prunable (dir gone), prunes and reattaches the
/// worktree to the still-existing branch. Otherwise creates fresh at the
/// trellis-convention path, branching from `start_point`.
///
/// `start_point` is only consulted when creating a NEW local branch. Callers
/// pass what they want the branch rooted at (e.g. `repo.default_branch`,
/// `session.base_branch`, or `format!("origin/{}", branch)` for PR review).
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

/// Freshen the default branch before branching from it. Returns the start
/// point for new branches and an optional warning.
///
/// After a good fetch, branches start from `origin/<default>` and local
/// `<default>` is fast-forwarded when that's safe, so a failed fast-forward
/// only warns. If the fetch fails (offline), fall back to the local branch.
pub fn refresh_default_branch(repo_path: &str, default_branch: &str) -> (String, Option<String>) {
    if let Err(e) = git::fetch_origin(repo_path) {
        return (
            default_branch.to_string(),
            Some(format!("{e}; branched from local {default_branch}")),
        );
    }
    let warning = git::fast_forward_branch(repo_path, default_branch).err().map(|e| e.to_string());
    (format!("origin/{default_branch}"), warning)
}

/// Detect subsystem directories in a monorepo (e.g. workers/model_train, src/sojourner).
pub fn detect_subsystems(repo_path: &str) -> Vec<String> {
    let root = Path::new(repo_path);
    let mut subsystems = Vec::new();
    for parent_name in &["workers", "src", "libs", "pods"] {
        let parent = root.join(parent_name);
        if parent.is_dir() {
            let mut children: Vec<_> = match std::fs::read_dir(&parent) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().is_dir()
                            && !e.file_name().to_string_lossy().starts_with('.')
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect(),
                Err(_) => continue,
            };
            children.sort();
            for child in children {
                subsystems.push(format!("{}/{}", parent_name, child));
            }
        }
    }
    subsystems
}

pub struct Manager {
    pub conn: Connection,
}

impl Manager {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    // ------------------------------------------------------------------
    // Properties
    // ------------------------------------------------------------------

    /// Returns the list of directories under which trellis looks for repos.
    /// Stored as a single config string with newline-separated paths so the
    /// value migrates seamlessly from the original single-dir setup. Defaults
    /// to `[~/dev]` if nothing is configured.
    pub fn repos_dirs(&self) -> Vec<PathBuf> {
        let raw = db::get_config(&self.conn, "repos_dir");
        let parsed: Vec<PathBuf> = raw
            .as_deref()
            .unwrap_or("")
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();
        if parsed.is_empty() {
            vec![dirs::home_dir().unwrap().join("dev")]
        } else {
            parsed
        }
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        db::get_config(&self.conn, "worktrees_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join("dev").join("worktrees"))
    }

    /// Command that launches the coding agent in a session's agent window.
    /// Must accept `-c` to continue the most recent conversation for its cwd
    /// (`pi`, `omp`, and `claude` all do).
    pub fn agent_cmd(&self) -> String {
        db::get_config(&self.conn, "agent_cmd")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_AGENT_CMD.to_string())
    }

    /// Agent for a session: the one picked at creation, else the global setting.
    pub fn agent_for(&self, session: &Session) -> String {
        session.agent.clone().unwrap_or_else(|| self.agent_cmd())
    }

    pub fn agent_for_name(&self, session_name: &str) -> String {
        match self.get_session_by_name(session_name) {
            Some(session) => self.agent_for(&session),
            None => self.agent_cmd(),
        }
    }

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

    /// If the tmux session for `session_id` is dead (DB row exists but
    /// `tmux has-session` is false), recreate it at the right working
    /// directory. No-op if the session is already alive.
    ///
    /// Working directory is resolved in this order:
    ///   1. A DB worktree row linked to this session via `session_id`
    ///   2. A DB worktree row matching `(repo_id, base_branch)` even if
    ///      `session_id` is `NULL` (e.g. scan_existing recorded the worktree
    ///      on disk but never linked it because no live tmux session existed
    ///      at scan time — the common case after reboot)
    ///   3. Git's authoritative `worktree list` for this repo, matching by
    ///      branch (covers worktrees created outside trellis)
    ///   4. The repo root (only correct for default-branch sessions)
    pub fn resurrect_session(&self, session_id: i64) -> Result<(), String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("session {} not found", session_id))?;

        if tmux::session_exists(&session.name) {
            return Ok(());
        }

        let repo = self
            .get_repo_by_id(session.repo_id)
            .ok_or_else(|| format!("repo {} not found", session.repo_id))?;

        let wt_path = db::get_worktrees_for_session(&self.conn, session_id)
            .into_iter()
            .next()
            .map(|wt| wt.path)
            .or_else(|| {
                db::get_worktrees(&self.conn)
                    .into_iter()
                    .find(|wt| wt.repo_id == session.repo_id && wt.branch == session.base_branch)
                    .map(|wt| wt.path)
            })
            .or_else(|| {
                git::list_worktrees(&repo.path).ok().and_then(|wts| {
                    wts.into_iter()
                        .find(|wt| wt.branch.as_deref() == Some(&session.base_branch))
                        .map(|wt| wt.path)
                })
            })
            .unwrap_or_else(|| repo.path.clone());

        if !Path::new(&wt_path).exists() {
            return Err(format!("path '{}' no longer exists", wt_path));
        }

        // Pick the conversation back up: every supported agent continues its
        // most recent session for the cwd with `-c`, and starts fresh if none.
        let revive_cmd = format!("{} -c", self.agent_for(&session));
        apply_layout(&session.name, &wt_path, &revive_cmd)
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;
        Ok(())
    }

    fn find_or_create_session(&self, repo: &Repo, branch: &str, wt_path: &str) -> Result<Session, String> {
        let session_name = tmux::sanitize_session_name(branch);

        // Existing DB row?
        if let Some(existing) = self.get_session_by_name(&session_name) {
            if tmux::session_exists(&session_name) {
                if existing.repo_id == repo.id.unwrap() {
                    return Ok(existing);
                }
                return Err(format!("Session '{}' is live for another repo", session_name));
            }
            // Dead row — drop it.
            if let Some(id) = existing.id {
                self.delete_session(id, false)?;
            }
        }

        // tmux first — fail loudly before touching the DB.
        apply_layout(&session_name, wt_path, &self.agent_cmd())
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;

        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
                agent: None,
            },
        );

        // A worktree row left behind by a deleted session has session_id NULL;
        // re-link it rather than inserting a duplicate.
        if let Some(wt) = db::get_worktree_by_path(&self.conn, wt_path) {
            if wt.session_id.is_none() {
                db::link_worktree_to_session(&self.conn, wt.id.unwrap(), session.id.unwrap());
            }
        } else if wt_path != repo.path {
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

        Ok(session)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn get_repo_by_id(&self, repo_id: i64) -> Option<Repo> {
        db::get_repos(&self.conn)
            .into_iter()
            .find(|r| r.id == Some(repo_id))
    }

    fn get_repo_by_path(&self, path: &str) -> Option<Repo> {
        db::get_repos(&self.conn)
            .into_iter()
            .find(|r| r.path == path)
    }

    fn get_session_by_id(&self, session_id: i64) -> Option<Session> {
        db::get_sessions(&self.conn)
            .into_iter()
            .find(|s| s.id == Some(session_id))
    }

    fn get_or_create_repo(&self, repo_path: &str) -> Repo {
        if let Some(repo) = self.get_repo_by_path(repo_path) {
            return repo;
        }
        let default_branch = git::detect_default_branch(repo_path).unwrap_or_else(|_| "main".into());
        let name = Path::new(repo_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        db::add_repo(
            &self.conn,
            &Repo {
                id: None,
                path: repo_path.to_string(),
                name,
                default_branch,
            },
        )
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Register repo if needed, create or reuse worktree, create or reuse DB
    /// session, create tmux session. If a live session for (repo, base_branch)
    /// already exists, routes to it (user-typed `session_name` ignored).
    pub fn create_session(
        &self,
        repo_path: &str,
        base_branch: &str,
        session_name: &str,
        subdirectory: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(Session, Option<String>), String> {
        let repo = self.get_or_create_repo(repo_path);

        // Spec: "n on branch X when X already has a session — route to the
        // existing session." Consult by branch BEFORE honoring the user-typed
        // name. The wizard's auto_session_name disambiguates to "feat-x-2",
        // "feat-x-3", etc. when a session for the branch exists; bypass that.
        let by_branch: Vec<Session> = db::get_sessions(&self.conn)
            .into_iter()
            .filter(|s| s.repo_id == repo.id.unwrap() && s.base_branch == base_branch)
            .collect();
        for s in by_branch {
            if tmux::session_exists(&s.name) {
                return Ok((s, None));
            }
            // Dead tmux session — drop stale row and continue.
            if let Some(id) = s.id {
                self.delete_session(id, false)?;
            }
        }

        // No live session for this branch. Resolve the worktree.
        let (start_point, warning) = refresh_default_branch(repo_path, &repo.default_branch);
        let wt_path = if base_branch == repo.default_branch {
            repo.path.clone()
        } else {
            // New branches root at the freshly fetched default branch.
            self.find_or_create_worktree(&repo, base_branch, &start_point)?
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

        // tmux first — fail loudly before touching the DB.
        let agent_cmd = agent.map(str::to_string).unwrap_or_else(|| self.agent_cmd());
        apply_layout(&chosen_name, &effective_dir, &agent_cmd)
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;

        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: chosen_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
                agent: agent.map(str::to_string),
            },
        );

        if let Some(wt) = db::get_worktree_by_path(&self.conn, &wt_path) {
            if wt.session_id.is_none() {
                db::link_worktree_to_session(&self.conn, wt.id.unwrap(), session.id.unwrap());
            }
        } else if wt_path != repo.path {
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

        Ok((session, warning))
    }

    /// Adopt an existing tmux session into trellis's management.
    pub fn adopt_session(
        &self,
        session_name: &str,
        repo_path: &str,
        base_branch: &str,
    ) -> Result<Session, String> {
        let repo = self.get_or_create_repo(repo_path);

        Ok(db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
                agent: None,
            },
        ))
    }

    /// Rename a session in both tmux and the DB.
    pub fn rename_session(&self, session_id: i64, new_name: &str) -> Result<Session, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        // Rename in tmux (if live)
        let _ = tmux::rename_session(&session.name, new_name);

        // Rename in DB
        self.conn
            .execute(
                "UPDATE sessions SET name = ?1 WHERE id = ?2",
                params![new_name, session_id],
            )
            .unwrap();

        Ok(Session {
            name: new_name.to_string(),
            ..session
        })
    }

    /// Update a session's base branch.
    pub fn set_base_branch(&self, session_id: i64, base_branch: &str) -> Result<Session, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        self.conn
            .execute(
                "UPDATE sessions SET base_branch = ?1 WHERE id = ?2",
                params![base_branch, session_id],
            )
            .unwrap();

        Ok(Session {
            base_branch: base_branch.to_string(),
            ..session
        })
    }

    /// Create or reuse worktree + session for a PR number or branch, ready
    /// for review. Returns (session, worktree_path).
    /// The network half of a PR review: resolve a PR number to its branch via
    /// `gh`, then fetch it. Two round trips, so callers run this on a worker —
    /// it needs no connection, only paths.
    pub fn resolve_review_branch(repo_path: &str, pr_or_branch: &str) -> Result<String, String> {
        let branch = if pr_or_branch.chars().all(|c| c.is_ascii_digit()) && !pr_or_branch.is_empty() {
            let pr_num: i64 = pr_or_branch.parse().unwrap();
            git::get_pr_branch(repo_path, pr_num).map_err(|e| format!("Failed to get PR branch: {}", e))?
        } else {
            pr_or_branch.to_string()
        };

        git::fetch_branch(repo_path, &branch);
        Ok(branch)
    }

    /// The local half, once the branch is fetched: worktree, session and db rows.
    pub fn checkout_and_review(
        &self,
        repo_path: &str,
        branch: &str,
    ) -> Result<(Session, String), String> {
        let repo = self.get_or_create_repo(repo_path);

        // PR review roots the new local branch at the PR's HEAD on the remote,
        // not main — preserves the original `checkout_and_review` semantic.
        let start_point = format!("origin/{}", branch);
        let wt_path = self.find_or_create_worktree(&repo, branch, &start_point)?;
        let session = self.find_or_create_session(&repo, branch, &wt_path)?;
        Ok((session, wt_path))
    }

    /// Insert the session and worktree rows for a freshly-adopted worktree.
    /// Private helper — `adopt_worktree` is the public entry point.
    fn persist_adopted_worktree(
        &self,
        repo_id: i64,
        path: &str,
        branch: &str,
        session_name: &str,
    ) -> Session {
        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id,
                base_branch: branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
                agent: None,
            },
        );
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id,
                session_id: session.id,
                path: path.to_string(),
                branch: branch.to_string(),
                tmux_window: None,
                created_at: utc_now(),
            },
        );
        session
    }

    /// Wrap a managed session around an existing on-disk worktree. Runs tmux
    /// first so a layout failure never leaves DB rows pointing at a
    /// non-existent tmux session.
    pub fn adopt_worktree(
        &self,
        repo_id: i64,
        path: &str,
        branch: &str,
        session_name: &str,
    ) -> Result<Session, String> {
        apply_layout(session_name, path, &self.agent_cmd())
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;
        Ok(self.persist_adopted_worktree(repo_id, path, branch, session_name))
    }

    /// Create or reuse a worktree + tmux window for the given session.
    /// If a worktree for `branch_name` already exists, attaches a new tmux
    /// window to it instead of erroring.
    pub fn add_tab(&self, session_id: i64, branch_name: &str) -> Result<Worktree, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let repo = self
            .get_repo_by_id(session.repo_id)
            .ok_or_else(|| format!("Repo {} not found", session.repo_id))?;

        // New branches in a tab root at the session's base branch (preserves existing behavior).
        let wt_path = self.find_or_create_worktree(&repo, branch_name, &session.base_branch)?;
        let _ = tmux::new_window(&session.name, branch_name, Some(&wt_path));

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

    /// Kill tmux session, optionally remove worktrees, remove DB session.
    pub fn delete_session(&self, session_id: i64, cleanup_worktrees: bool) -> Result<(), String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let repo = self.get_repo_by_id(session.repo_id);

        if cleanup_worktrees {
            let worktrees = db::get_worktrees_for_session(&self.conn, session_id);
            for wt in &worktrees {
                if let Some(ref r) = repo {
                    let _ = git::remove_worktree(&r.path, &wt.path);
                }
                db::delete_worktree(&self.conn, wt.id.unwrap());
            }
        } else {
            // Detach worktrees from this session to satisfy FK constraint
            self.conn
                .execute(
                    "UPDATE worktrees SET session_id = NULL WHERE session_id = ?1",
                    params![session_id],
                )
                .unwrap();
        }

        let _ = tmux::kill_session(&session.name);
        db::delete_session(&self.conn, session_id);

        Ok(())
    }

    /// Drop the DB record for a worktree. The git removal is the slow half and
    /// runs on a worker thread; this is the commit the main thread applies once
    /// the worker reports the directory is gone.
    pub fn forget_worktree(&self, worktree_id: i64) {
        db::delete_worktree(&self.conn, worktree_id);
    }

    /// Return DB sessions enriched with live tmux state, plus unmanaged live sessions.
    /// Enrich the db's sessions with live tmux state, plus any live sessions the
    /// db doesn't know about. Takes an already-gathered tmux listing so the
    /// process spawn happens on a worker, not the event loop.
    pub fn list_sessions_with(&self, live_list: &[tmux::TmuxSession]) -> Vec<SessionInfo> {
        let db_sessions = db::get_sessions(&self.conn);
        let live_by_name: HashMap<String, &tmux::TmuxSession> = live_list
            .iter()
            .map(|s| (s.name.clone(), s))
            .collect();
        let mut db_names: HashSet<String> = HashSet::new();

        let mut result: Vec<SessionInfo> = Vec::new();
        for session in &db_sessions {
            db_names.insert(session.name.clone());
            let live = live_by_name.get(&session.name);
            result.push(SessionInfo {
                id: session.id,
                name: session.name.clone(),
                repo_id: Some(session.repo_id),
                base_branch: Some(session.base_branch.clone()),
                created_at: Some(session.created_at.clone()),
                last_selected_at: session.last_selected_at.clone(),
                windows: live.map(|l| l.windows),
                attached: live.map(|l| l.attached).unwrap_or(false),
                live: live.is_some(),
                managed: true,
            });
        }

        // Include live tmux sessions not tracked in the DB
        for ts in live_list {
            if !db_names.contains(&ts.name) {
                result.push(SessionInfo {
                    id: None,
                    name: ts.name.clone(),
                    repo_id: None,
                    base_branch: None,
                    created_at: None,
                    last_selected_at: None,
                    windows: Some(ts.windows),
                    attached: ts.attached,
                    live: true,
                    managed: false,
                });
            }
        }

        result
    }

    // ------------------------------------------------------------------
    // scan_existing and sub-scans
    // ------------------------------------------------------------------

    fn scan_repos(&self, home_dev: &Path, known_repos: &mut HashMap<String, Repo>) {
        for path in git::find_repos(home_dev, &self.worktrees_dir(), git::REPO_SCAN_DEPTH) {
            let path_str = path.to_string_lossy().to_string();
            if known_repos.contains_key(&path_str) {
                continue;
            }
            let default_branch =
                git::detect_default_branch(&path_str).unwrap_or_else(|_| "main".into());
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let repo = db::add_repo(
                &self.conn,
                &Repo {
                    id: None,
                    path: path_str.clone(),
                    name,
                    default_branch,
                },
            );
            known_repos.insert(path_str, repo);
        }
    }

    fn scan_worktrees(
        &self,
        home_dev: &Path,
        worktrees_root: &Path,
        known_repos: &mut HashMap<String, Repo>,
        known_worktree_paths: &mut HashSet<String>,
    ) {
        if !worktrees_root.is_dir() {
            return;
        }
        let repo_dirs = match std::fs::read_dir(worktrees_root) {
            Ok(e) => e,
            Err(_) => return,
        };
        for repo_entry in repo_dirs.filter_map(|e| e.ok()) {
            let repo_dir = repo_entry.path();
            if !repo_dir.is_dir() {
                continue;
            }
            let branch_dirs = match std::fs::read_dir(&repo_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for branch_entry in branch_dirs.filter_map(|e| e.ok()) {
                let branch_dir = branch_entry.path();
                if !branch_dir.is_dir() {
                    continue;
                }
                let branch_dir_str = branch_dir.to_string_lossy().to_string();
                if known_worktree_paths.contains(&branch_dir_str) {
                    continue;
                }

                let repo_dir_name = repo_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                // Try this dir first, then fall back to looking up by name
                // across known_repos (covers the case where the repo lives in
                // a different configured repos_dir).
                let repo_path_candidate = home_dev.join(&repo_dir_name).to_string_lossy().to_string();
                let already_known = known_repos.contains_key(&repo_path_candidate)
                    || known_repos.values().any(|r| r.name == repo_dir_name);

                if !already_known {
                    let candidate_path = Path::new(&repo_path_candidate);
                    if candidate_path.is_dir() {
                        let default_branch = git::detect_default_branch(&repo_path_candidate)
                            .unwrap_or_else(|_| "main".into());
                        let repo = db::add_repo(
                            &self.conn,
                            &Repo {
                                id: None,
                                path: repo_path_candidate.clone(),
                                name: repo_dir_name.clone(),
                                default_branch,
                            },
                        );
                        known_repos.insert(repo_path_candidate.clone(), repo);
                    } else {
                        continue;
                    }
                }

                // Look up the repo by exact path first, then by name. The
                // path lookup covers freshly-adopted repos in this same
                // scan_worktrees call; the name lookup covers repos found in
                // a different configured repos_dir.
                let repo = match known_repos.get(&repo_path_candidate) {
                    Some(r) => r,
                    None => match known_repos.values().find(|r| r.name == repo_dir_name) {
                        Some(r) => r,
                        None => continue,
                    },
                };
                let branch_name = branch_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                db::add_worktree(
                    &self.conn,
                    &Worktree {
                        id: None,
                        repo_id: repo.id.unwrap(),
                        path: branch_dir_str.clone(),
                        branch: branch_name,
                        session_id: None,
                        tmux_window: None,
                        created_at: utc_now(),
                    },
                );
                known_worktree_paths.insert(branch_dir_str);
            }
        }
    }

    /// For each managed session, check its tmux windows for panes sitting in
    /// worktree directories and adopt them (create or link DB records).
    fn adopt_session_worktrees(
        &self,
        worktrees_root: &Path,
        known_repos: &HashMap<String, Repo>,
    ) {
        let sessions = db::get_sessions(&self.conn);
        let all_windows = tmux::list_all_windows();

        for session in &sessions {
            let windows = match all_windows.get(&session.name) {
                Some(w) => w,
                None => continue,
            };

            for win in windows {
                let pane_path = Path::new(&win.path);

                // Check if this pane is inside the worktrees directory
                let rel = match pane_path.strip_prefix(worktrees_root) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                // Expected structure: <repo_name>/<branch_name>[/...]
                let mut components = rel.components();
                let repo_name = match components.next() {
                    Some(c) => c.as_os_str().to_string_lossy().to_string(),
                    None => continue,
                };
                let branch_dir = match components.next() {
                    Some(c) => c.as_os_str().to_string_lossy().to_string(),
                    None => continue,
                };

                // The worktree path is worktrees_root/<repo>/<branch>
                let wt_path = worktrees_root
                    .join(&repo_name)
                    .join(&branch_dir)
                    .to_string_lossy()
                    .to_string();

                if let Some(existing) = db::get_worktree_by_path(&self.conn, &wt_path) {
                    // Already tracked — link to this session if orphaned
                    if existing.session_id.is_none() {
                        db::link_worktree_to_session(
                            &self.conn,
                            existing.id.unwrap(),
                            session.id.unwrap(),
                        );
                    }
                } else {
                    // Find the repo for this worktree — match by name across
                    // all known repos rather than constructing a candidate
                    // path under a single repos_dir.
                    let repo = match known_repos.values().find(|r| r.name == repo_name) {
                        Some(r) => r,
                        None => continue,
                    };

                    db::add_worktree(
                        &self.conn,
                        &Worktree {
                            id: None,
                            repo_id: repo.id.unwrap(),
                            path: wt_path,
                            branch: branch_dir,
                            session_id: session.id,
                            tmux_window: Some(win.index),
                            created_at: utc_now(),
                        },
                    );
                }
            }
        }
    }

    fn scan_tmux_sessions(&self, known_repos: &HashMap<String, Repo>) {
        let known_session_names: HashSet<String> = db::get_sessions(&self.conn)
            .into_iter()
            .map(|s| s.name)
            .collect();

        for ts in tmux::list_sessions() {
            if known_session_names.contains(&ts.name) {
                continue;
            }
            let mut matched_repo: Option<&Repo> = None;
            for repo in known_repos.values() {
                if ts.name.starts_with(&repo.name) {
                    matched_repo = Some(repo);
                    break;
                }
            }
            if let Some(repo) = matched_repo {
                db::add_session(
                    &self.conn,
                    &Session {
                        id: None,
                        name: ts.name.clone(),
                        repo_id: repo.id.unwrap(),
                        base_branch: repo.default_branch.clone(),
                        created_at: utc_now(),
                        last_selected_at: None,
                        agent: None,
                    },
                );
            }
        }
    }

    /// First-run adoption: scan repos and worktrees dirs, scan live tmux sessions,
    /// and populate the DB with discovered state.
    pub fn scan_existing(&self) {
        let repos_dirs = self.repos_dirs();
        let worktrees_root = self.worktrees_dir();
        let mut known_repos: HashMap<String, Repo> = db::get_repos(&self.conn)
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();
        let mut known_worktree_paths: HashSet<String> = db::get_worktrees(&self.conn)
            .into_iter()
            .map(|wt| wt.path)
            .collect();

        for dir in &repos_dirs {
            self.scan_repos(dir, &mut known_repos);
        }
        for dir in &repos_dirs {
            self.scan_worktrees(dir, &worktrees_root, &mut known_repos, &mut known_worktree_paths);
        }
        self.scan_tmux_sessions(&known_repos);
        self.adopt_session_worktrees(&worktrees_root, &known_repos);
    }

    // ------------------------------------------------------------------
    // Public convenience wrappers
    // ------------------------------------------------------------------

    pub fn get_repos(&self) -> Vec<Repo> {
        db::get_repos(&self.conn)
    }

    pub fn get_sessions(&self) -> Vec<Session> {
        db::get_sessions(&self.conn)
    }

    pub fn get_worktrees_for_session(&self, session_id: i64) -> Vec<Worktree> {
        db::get_worktrees_for_session(&self.conn, session_id)
    }

    pub fn touch_session(&self, session_id: i64) {
        db::touch_session(&self.conn, session_id);
    }

    pub fn get_session_by_name(&self, name: &str) -> Option<Session> {
        db::get_session_by_name(&self.conn, name)
    }

    pub fn get_all_config(&self) -> Vec<(String, String)> {
        db::get_all_config(&self.conn)
    }

    pub fn set_config(&self, key: &str, value: &str) {
        db::set_config(&self.conn, key, value);
    }

    pub fn get_all_worktrees(&self) -> Vec<Worktree> {
        db::get_worktrees(&self.conn)
    }

    /// Discover worktrees git knows about (per registered repo) that Trellis
    /// doesn't track. Filters out missing directories on disk before delegating
    /// the data filter to `filter_orphans`.
    pub fn list_orphan_worktrees(&self) -> Vec<OrphanWorktree> {
        let repos = db::get_repos(&self.conn);
        let tracked_paths: HashSet<String> = db::get_worktrees(&self.conn)
            .into_iter()
            .map(|wt| wt.path)
            .collect();
        let mut git_results: HashMap<i64, Vec<GitWorktree>> = HashMap::new();
        for repo in &repos {
            let Some(repo_id) = repo.id else { continue };
            let entries = match git::list_worktrees(&repo.path) {
                Ok(es) => es,
                Err(_) => continue, // skip repos whose git invocation fails
            };
            // IO-side filter: drop entries whose directory no longer exists on
            // disk. `git worktree list` reports them until `git worktree prune`
            // runs.
            let live: Vec<GitWorktree> = entries
                .into_iter()
                .filter(|e| Path::new(&e.path).is_dir())
                .collect();
            git_results.insert(repo_id, live);
        }
        filter_orphans(&repos, &tracked_paths, &git_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitWorktree;
    use rusqlite::Connection;

    fn make_test_manager() -> Manager {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn);
        Manager::new(conn)
    }

    fn make_repo(id: i64, name: &str, path: &str) -> Repo {
        Repo {
            id: Some(id),
            path: path.to_string(),
            name: name.to_string(),
            default_branch: "main".to_string(),
        }
    }

    fn make_gw(path: &str, branch: Option<&str>) -> GitWorktree {
        GitWorktree {
            path: path.to_string(),
            branch: branch.map(|b| b.to_string()),
            commit: None,
        }
    }

    #[test]
    fn filter_orphans_excludes_main_worktree() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(1, vec![make_gw("/dev/trellis", Some("main"))]);
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_excludes_tracked_paths() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(1, vec![make_gw("/wt/trellis/feat-a", Some("feat-a"))]);
        let mut tracked = HashSet::new();
        tracked.insert("/wt/trellis/feat-a".to_string());
        let result = filter_orphans(&[repo], &tracked, &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_excludes_detached_head() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(1, vec![make_gw("/wt/trellis/probe", Some("(detached)"))]);
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_excludes_missing_branch_defensively() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(1, vec![make_gw("/wt/trellis/x", None)]);
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_includes_untracked_branch() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![
                make_gw("/dev/trellis", Some("main")),
                make_gw("/wt/trellis/feat-a", Some("feat-a")),
            ],
        );
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].branch, "feat-a");
        assert_eq!(result[0].path, "/wt/trellis/feat-a");
        assert_eq!(result[0].repo.name, "trellis");
    }

    #[test]
    fn filter_orphans_sorts_by_repo_then_branch() {
        let r1 = make_repo(1, "alpha", "/dev/alpha");
        let r2 = make_repo(2, "beta", "/dev/beta");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![
                make_gw("/wt/alpha/zeta", Some("zeta")),
                make_gw("/wt/alpha/aaa", Some("aaa")),
            ],
        );
        git_results.insert(2, vec![make_gw("/wt/beta/mid", Some("mid"))]);
        let result = filter_orphans(&[r1, r2], &HashSet::new(), &git_results);
        let names: Vec<(String, String)> = result
            .iter()
            .map(|o| (o.repo.name.clone(), o.branch.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("alpha".to_string(), "aaa".to_string()),
                ("alpha".to_string(), "zeta".to_string()),
                ("beta".to_string(), "mid".to_string()),
            ]
        );
    }

    #[test]
    fn filter_orphans_skips_repos_without_git_results() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let git_results: HashMap<i64, Vec<GitWorktree>> = HashMap::new();
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn persist_adopted_worktree_writes_session_and_worktree_rows() {
        let mgr = make_test_manager();
        let repo = db::add_repo(
            &mgr.conn,
            &Repo {
                id: None,
                path: "/dev/trellis".into(),
                name: "trellis".into(),
                default_branch: "main".into(),
            },
        );
        let session = mgr.persist_adopted_worktree(
            repo.id.unwrap(),
            "/wt/trellis/feat-a",
            "feat-a",
            "trellis-feat-a",
        );

        assert_eq!(session.name, "trellis-feat-a");
        assert_eq!(session.repo_id, repo.id.unwrap());
        assert_eq!(session.base_branch, "feat-a");
        let from_db = db::get_session_by_name(&mgr.conn, "trellis-feat-a").unwrap();
        assert_eq!(from_db.id, session.id);

        let worktrees = db::get_worktrees_for_session(&mgr.conn, session.id.unwrap());
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, "/wt/trellis/feat-a");
        assert_eq!(worktrees[0].branch, "feat-a");
        assert_eq!(worktrees[0].repo_id, repo.id.unwrap());
        assert_eq!(worktrees[0].session_id, session.id);
    }
}

#[cfg(test)]
mod reuse_tests {
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
        let canonical = tmp.canonicalize().unwrap();
        let repo = canonical.join("repo");
        let worktrees = canonical.join("worktrees");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&worktrees).unwrap();
        init_repo(&repo);
        (repo.to_string_lossy().to_string(), worktrees.to_string_lossy().to_string())
    }

    #[test]
    fn resolve_creates_when_no_existing_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let path = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        assert!(Path::new(&path).exists());
        assert!(path.ends_with("feat-x"));
    }

    #[test]
    fn resolve_reuses_existing_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let first = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        let second = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn resolve_recovers_from_prunable_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let first = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        // Nuke the dir behind git's back to make it prunable.
        fs::remove_dir_all(&first).unwrap();
        let second = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        assert!(Path::new(&second).exists());
    }

    #[test]
    fn resolve_finds_worktree_at_nonconventional_path() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        // User created a worktree at an unusual path outside our convention.
        let unusual = tmp.path().canonicalize().unwrap().join("unusual-spot");
        Command::new("git")
            .args(["worktree", "add", "-b", "feat-y", unusual.to_str().unwrap(), "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        // Now ask trellis to find/create for feat-y — should return the unusual path.
        let found = resolve_or_create_worktree(&repo, "myrepo", "feat-y", &wts, "main").unwrap();
        assert_eq!(found, unusual.to_string_lossy().to_string());
    }

    // ------------------------------------------------------------------
    // find_or_create_session tests
    // ------------------------------------------------------------------

    fn make_manager() -> Manager {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn);
        // Keep tests from launching a real agent in their tmux sessions.
        crate::db::set_config(&conn, "agent_cmd", "true");
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
    fn review_input_that_is_not_a_pr_number_is_used_as_the_branch_verbatim() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());

        // Non-numeric input must not be sent to `gh`; it is already a branch.
        // The fetch that follows is best-effort and fails silently with no remote.
        let branch = Manager::resolve_review_branch(&repo_path, "feat/some-branch").unwrap();

        assert_eq!(branch, "feat/some-branch");
    }

    #[test]
    fn review_input_with_a_slash_is_never_treated_as_a_pr_number() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());

        // Guards the digit test against matching things like "123/foo".
        let branch = Manager::resolve_review_branch(&repo_path, "123/foo").unwrap();

        assert_eq!(branch, "123/foo");
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
    fn repos_dirs_parses_multiline_config() {
        let mgr = make_manager();
        crate::db::set_config(&mgr.conn, "repos_dir", "/Users/x/personal\n/Users/x/work");
        let dirs = mgr.repos_dirs();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/Users/x/personal"),
                PathBuf::from("/Users/x/work"),
            ]
        );
    }

    #[test]
    fn repos_dirs_ignores_blank_lines_and_trims() {
        let mgr = make_manager();
        crate::db::set_config(&mgr.conn, "repos_dir", "  /a  \n\n  /b  \n");
        let dirs = mgr.repos_dirs();
        assert_eq!(dirs, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn repos_dirs_falls_back_to_default_when_blank() {
        let mgr = make_manager();
        crate::db::set_config(&mgr.conn, "repos_dir", "");
        let dirs = mgr.repos_dirs();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("dev"));
    }

    #[test]
    fn resurrect_session_creates_dead_tmux_session() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();

        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("resurrect");
        let session_name = crate::tmux::sanitize_session_name(&branch);
        let _guard = SessionGuard(session_name.clone());

        // Insert a DB session + worktree but never start the tmux session.
        let inserted = crate::db::add_session(&mgr.conn, &crate::models::Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.clone(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
            agent: None,
        });
        crate::db::add_worktree(&mgr.conn, &crate::models::Worktree {
            id: None,
            repo_id: repo.id.unwrap(),
            path: repo_path.clone(),
            branch: branch.clone(),
            session_id: inserted.id,
            tmux_window: None,
            created_at: crate::utils::utc_now(),
        });
        assert!(!crate::tmux::session_exists(&session_name));

        mgr.resurrect_session(inserted.id.unwrap()).unwrap();
        assert!(crate::tmux::session_exists(&session_name));
    }

    #[test]
    fn apply_layout_creates_named_windows_independent_of_base_index() {
        // The bug we're fixing: hardcoded window index 1 in rename_window
        // silently failed on default tmux (base-index 0), leaving the first
        // window with tmux's default name ("zsh") instead of the agent window.
        // The fix passes -n at new-session time, so the first window is
        // correctly named regardless of base-index.
        let tmp = tempdir().unwrap();
        let session_name = format!("trellistest-layout-{}", std::process::id());
        let _guard = SessionGuard(session_name.clone());

        apply_layout(&session_name, &tmp.path().to_string_lossy(), "true").unwrap();
        assert!(crate::tmux::session_exists(&session_name));

        let windows = crate::tmux::list_windows(&session_name);
        let names: Vec<String> = windows.iter().map(|w| w.name.clone()).collect();
        assert_eq!(names, vec![AGENT_WINDOW.to_string(), "shell".to_string()]);
    }

    #[test]
    fn resurrect_session_uses_orphan_worktree_by_branch() {
        // The scenario the user hit on 2026-05-19: scan_existing recorded a
        // worktree on disk but left session_id NULL. After reboot, the DB has
        // a session row + an orphan worktree row matching by (repo_id,
        // base_branch). resurrect_session must find that worktree and use its
        // path, not fall back to the repo root.
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();

        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("orphan");
        let session_name = crate::tmux::sanitize_session_name(&branch);
        let _guard = SessionGuard(session_name.clone());

        // A separate, real on-disk path for the worktree (not the repo root).
        let worktree_dir = tmp.path().canonicalize().unwrap().join("wt-orphan");
        std::fs::create_dir_all(&worktree_dir).unwrap();
        let wt_path_str = worktree_dir.to_string_lossy().to_string();

        let inserted = crate::db::add_session(&mgr.conn, &crate::models::Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.clone(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
            agent: None,
        });
        crate::db::add_worktree(&mgr.conn, &crate::models::Worktree {
            id: None,
            repo_id: repo.id.unwrap(),
            path: wt_path_str.clone(),
            branch: branch.clone(),
            session_id: None, // <- orphan: not linked to the session row above
            tmux_window: None,
            created_at: crate::utils::utc_now(),
        });

        mgr.resurrect_session(inserted.id.unwrap()).unwrap();
        assert!(crate::tmux::session_exists(&session_name));

        // Inspect tmux to confirm the working dir is the worktree, not the repo root.
        let out = std::process::Command::new("tmux")
            .args(["display-message", "-p", "-t", &session_name, "#{session_path}"])
            .output()
            .unwrap();
        let session_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(session_path, wt_path_str);
    }

    #[test]
    fn resurrect_session_is_noop_when_already_alive() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();

        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("alive");
        let session_name = crate::tmux::sanitize_session_name(&branch);
        let _guard = SessionGuard(session_name.clone());

        // Pre-create the tmux session so resurrect_session sees a live one.
        crate::tmux::new_session(&session_name, &repo_path, None).unwrap();
        assert!(crate::tmux::session_exists(&session_name));

        let inserted = crate::db::add_session(&mgr.conn, &crate::models::Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.clone(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
            agent: None,
        });

        mgr.resurrect_session(inserted.id.unwrap()).unwrap();
        // Still alive, never killed and recreated.
        assert!(crate::tmux::session_exists(&session_name));
    }

    #[test]
    fn resurrect_session_errors_when_worktree_path_missing() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();

        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("missing-path");
        let session_name = crate::tmux::sanitize_session_name(&branch);
        let _guard = SessionGuard(session_name.clone());

        let inserted = crate::db::add_session(&mgr.conn, &crate::models::Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.clone(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
            agent: None,
        });
        // Worktree row points at a path that doesn't exist.
        crate::db::add_worktree(&mgr.conn, &crate::models::Worktree {
            id: None,
            repo_id: repo.id.unwrap(),
            path: "/var/empty/does-not-exist-trellistest".to_string(),
            branch: branch.clone(),
            session_id: inserted.id,
            tmux_window: None,
            created_at: crate::utils::utc_now(),
        });

        let result = mgr.resurrect_session(inserted.id.unwrap());
        assert!(result.is_err());
        assert!(!crate::tmux::session_exists(&session_name));
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
            agent: None,
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

    #[test]
    fn scan_repos_registers_repo_nested_in_container_dir() {
        let mgr = make_manager();
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("group/proj");
        std::fs::create_dir_all(nested.join(".git")).unwrap();
        let mut known = HashMap::new();
        mgr.scan_repos(tmp.path(), &mut known);
        assert!(known.contains_key(nested.to_str().unwrap()));
        assert_eq!(crate::db::get_repos(&mgr.conn).len(), 1);
    }

    #[test]
    fn agent_for_prefers_the_session_agent_over_the_global_setting() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();
        let repo = mgr.get_or_create_repo(&repo_path);
        let mut session = crate::models::Session {
            id: None,
            name: "agent-for".into(),
            repo_id: repo.id.unwrap(),
            base_branch: "main".into(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
            agent: None,
        };
        assert_eq!(mgr.agent_for(&session), mgr.agent_cmd());
        session.agent = Some("pi".into());
        assert_eq!(mgr.agent_for(&session), "pi");

        crate::db::add_session(&mgr.conn, &session);
        assert_eq!(mgr.agent_for_name("agent-for"), "pi");
        assert_eq!(mgr.agent_for_name("unknown"), mgr.agent_cmd());
    }

    #[test]
    fn resurrect_relaunches_the_sessions_own_agent() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();
        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("agent-resume");
        let session_name = crate::tmux::sanitize_session_name(&branch);
        let _guard = SessionGuard(session_name.clone());

        let inserted = crate::db::add_session(&mgr.conn, &crate::models::Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.clone(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
            agent: Some("printf trellis-session-agent".into()),
        });
        crate::db::add_worktree(&mgr.conn, &crate::models::Worktree {
            id: None,
            repo_id: repo.id.unwrap(),
            path: repo_path.clone(),
            branch,
            session_id: inserted.id,
            tmux_window: None,
            created_at: crate::utils::utc_now(),
        });
        mgr.resurrect_session(inserted.id.unwrap()).unwrap();

        let target = format!("{}:{}", session_name, AGENT_WINDOW);
        let mut pane = String::new();
        for _ in 0..20 {
            pane = String::from_utf8_lossy(
                &Command::new("tmux").args(["capture-pane", "-p", "-t", &target]).output().unwrap().stdout,
            )
            .to_string();
            if pane.contains("trellis-session-agent -c") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(pane.contains("trellis-session-agent -c"), "pane was: {pane}");
    }

    #[test]
    fn create_session_records_the_chosen_agent() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();
        let name = unique_branch("agent-create");
        let _guard = SessionGuard(crate::tmux::sanitize_session_name(&name));

        let (session, _) = mgr.create_session(&repo_path, "main", &name, None, Some("pi")).unwrap();
        assert_eq!(session.agent.as_deref(), Some("pi"));
        assert_eq!(mgr.agent_for_name(&session.name), "pi");
    }

    #[test]
    fn create_session_branches_from_fresh_origin_and_fast_forwards_local_main() {
        use crate::git::tests::{git_out, repo_behind_origin};
        let tmp = tempdir().unwrap();
        let (repo, tip) = repo_behind_origin(tmp.path());
        // Root checkout sits on another branch, like a user mid-work.
        git_out(&repo, &["checkout", "-q", "-b", "work"]);
        let work_before = git_out(&repo, &["rev-parse", "work"]);
        let wts = tmp.path().canonicalize().unwrap().join("wts");

        let mgr = make_manager();
        crate::db::set_config(&mgr.conn, "worktrees_dir", wts.to_str().unwrap());
        let branch = unique_branch("fresh-origin");
        let _guard = SessionGuard(crate::tmux::sanitize_session_name(&branch));

        let (_, warning) = mgr
            .create_session(repo.to_str().unwrap(), &branch, "", None, None)
            .unwrap();

        assert_eq!(warning, None);
        let wt = wts.join("repo").join(branch.replace('/', "-"));
        assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), tip);
        assert_eq!(git_out(&repo, &["rev-parse", "main"]), tip);
        assert_eq!(git_out(&repo, &["rev-parse", "work"]), work_before);
        assert_eq!(git_out(&repo, &["branch", "--show-current"]), "work");
        let tracking = Command::new("git")
            .args(["config", "--get", &format!("branch.{branch}.remote")])
            .current_dir(&wt)
            .output()
            .unwrap();
        assert!(!tracking.status.success(), "new branch must not track origin/main");
    }

    #[test]
    fn refresh_falls_back_to_local_branch_with_a_warning_when_offline() {
        use crate::git::tests::{git_out, repo_behind_origin};
        let tmp = tempdir().unwrap();
        let (repo, _) = repo_behind_origin(tmp.path());
        git_out(&repo, &["remote", "set-url", "origin", "/nonexistent/origin.git"]);

        let (start, warning) = refresh_default_branch(repo.to_str().unwrap(), "main");
        assert_eq!(start, "main");
        assert!(warning.unwrap().contains("local main"));
    }

    #[test]
    fn refresh_still_branches_from_origin_when_fast_forward_is_refused() {
        use crate::git::tests::{git_out, repo_behind_origin};
        let tmp = tempdir().unwrap();
        let (repo, _) = repo_behind_origin(tmp.path());
        git_out(&repo, &["commit", "--allow-empty", "-qm", "local-only"]);

        let (start, warning) = refresh_default_branch(repo.to_str().unwrap(), "main");
        assert_eq!(start, "origin/main");
        assert!(warning.is_some());
    }

    #[test]
    fn create_session_starts_from_origin_even_when_local_main_diverged() {
        use crate::git::tests::{git_out, repo_behind_origin};
        let tmp = tempdir().unwrap();
        let (repo, tip) = repo_behind_origin(tmp.path());
        git_out(&repo, &["checkout", "-q", "-b", "work"]);
        git_out(&repo, &["commit", "--allow-empty", "-qm", "local-only"]);
        git_out(&repo, &["branch", "-f", "main", "work"]);
        let main_before = git_out(&repo, &["rev-parse", "main"]);
        let wts = tmp.path().canonicalize().unwrap().join("wts");

        let mgr = make_manager();
        crate::db::set_config(&mgr.conn, "worktrees_dir", wts.to_str().unwrap());
        let branch = unique_branch("diverged");
        let _guard = SessionGuard(crate::tmux::sanitize_session_name(&branch));

        let (_, warning) = mgr
            .create_session(repo.to_str().unwrap(), &branch, "", None, None)
            .unwrap();

        assert!(warning.is_some());
        let wt = wts.join("repo").join(branch.replace('/', "-"));
        assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), tip);
        assert_eq!(git_out(&repo, &["rev-parse", "main"]), main_before);
    }
}
