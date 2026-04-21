// trellis/src/manager.rs
//! High-level orchestration: ties DB, tmux, and git together.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::db;
use crate::git;
use crate::models::{Repo, Session, SessionInfo, Worktree};
use crate::tmux;

fn utc_now() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

/// Default tmux layout: (window_name, optional command to send).
const DEFAULT_LAYOUT: &[(&str, Option<&str>)] = &[
    ("claude", Some("claude")),
    ("shell", None),
];

/// Create a tmux session and set up windows according to the default layout.
pub fn apply_layout(session_name: &str, working_dir: &str) {
    // Create the session
    if let Err(_) = tmux::new_session(session_name, working_dir) {
        return;
    }

    for (i, &(name, command)) in DEFAULT_LAYOUT.iter().enumerate() {
        if i == 0 {
            let _ = tmux::rename_window(session_name, 1, name);
        } else {
            let _ = tmux::new_window(session_name, name, Some(working_dir));
        }
        if let Some(cmd) = command {
            let target = format!("{}:{}", session_name, name);
            tmux::send_keys(&target, &[cmd, "Enter"]);
        }
    }

    // Focus the first window
    let _ = tmux::select_window(session_name, 1);
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

    pub fn repos_dir(&self) -> PathBuf {
        db::get_config(&self.conn, "repos_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join("dev"))
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        db::get_config(&self.conn, "worktrees_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join("dev").join("worktrees"))
    }

    pub fn worktree_path(&self, repo_name: &str, branch: &str) -> String {
        self.worktrees_dir()
            .join(repo_name)
            .join(branch.replace("/", "-"))
            .to_string_lossy()
            .to_string()
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

    fn get_worktree_by_id(&self, worktree_id: i64) -> Option<Worktree> {
        db::get_worktrees(&self.conn)
            .into_iter()
            .find(|wt| wt.id == Some(worktree_id))
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

    /// Register repo if needed, create worktree, create DB session, create tmux session.
    pub fn create_session(
        &self,
        repo_path: &str,
        base_branch: &str,
        session_name: &str,
        subdirectory: Option<&str>,
    ) -> Session {
        let repo = self.get_or_create_repo(repo_path);

        let default = &repo.default_branch;
        let start_dir = if base_branch == default {
            repo_path.to_string()
        } else {
            // Fetch latest before creating worktree
            git::fetch_and_pull(repo_path, default);

            let wt_path = self.worktree_path(&repo.name, base_branch);
            match git::create_worktree(repo_path, &wt_path, base_branch, default) {
                Ok(()) => {}
                Err(_) => {
                    // Branch/worktree may already exist - use it if the dir is there
                    if !Path::new(&wt_path).exists() {
                        panic!("Worktree creation failed and path does not exist");
                    }
                }
            }
            wt_path
        };

        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );

        let effective_dir = if let Some(sub) = subdirectory {
            Path::new(&start_dir).join(sub).to_string_lossy().to_string()
        } else {
            start_dir.clone()
        };

        apply_layout(session_name, &effective_dir);

        // Record worktree if we created one
        if start_dir != repo_path {
            db::add_worktree(
                &self.conn,
                &Worktree {
                    id: None,
                    repo_id: repo.id.unwrap(),
                    path: start_dir,
                    branch: base_branch.to_string(),
                    session_id: session.id,
                    tmux_window: None,
                    created_at: utc_now(),
                },
            );
        }

        session
    }

    /// Adopt an existing tmux session into trellis's management.
    pub fn adopt_session(
        &self,
        session_name: &str,
        repo_path: &str,
        base_branch: &str,
    ) -> Session {
        let repo = self.get_or_create_repo(repo_path);

        db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        )
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

    /// Create a worktree + session for a PR number or branch, ready for review.
    /// Returns (session, worktree_path). Reuses existing if found.
    pub fn checkout_and_review(
        &self,
        repo_path: &str,
        pr_or_branch: &str,
    ) -> Result<(Session, String), String> {
        let repo = self.get_or_create_repo(repo_path);

        // Resolve PR number to branch name
        let branch = if pr_or_branch.chars().all(|c| c.is_ascii_digit()) && !pr_or_branch.is_empty()
        {
            let pr_num: i64 = pr_or_branch.parse().unwrap();
            git::get_pr_branch(repo_path, pr_num)
                .map_err(|e| format!("Failed to get PR branch: {}", e))?
        } else {
            pr_or_branch.to_string()
        };

        // Fetch the branch so it's available locally
        git::fetch_branch(repo_path, &branch);

        // Check if we already have a session for this branch
        let session_name = tmux::sanitize_session_name(&branch);
        let wt_path = self.worktree_path(&repo.name, &branch);
        let existing_session = self.get_session_by_name(&session_name);

        if let Some(ref s) = existing_session {
            if Path::new(&wt_path).exists() {
                return Ok((s.clone(), wt_path));
            }
        }

        // Create worktree if it doesn't already exist on disk
        if !Path::new(&wt_path).exists() {
            let base = format!("origin/{}", branch);
            git::create_worktree(repo_path, &wt_path, &branch, &base)
                .map_err(|e| format!("Failed to create worktree: {}", e))?;
        }

        if let Some(s) = existing_session {
            return Ok((s, wt_path));
        }

        // Create session
        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: branch.clone(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );
        apply_layout(&session_name, &wt_path);

        // Record worktree
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id: repo.id.unwrap(),
                path: wt_path.clone(),
                branch: branch.clone(),
                session_id: session.id,
                tmux_window: None,
                created_at: utc_now(),
            },
        );

        Ok((session, wt_path))
    }

    /// Create a worktree + tmux window for the given session.
    pub fn add_tab(&self, session_id: i64, branch_name: &str) -> Result<Worktree, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let repo = self
            .get_repo_by_id(session.repo_id)
            .ok_or_else(|| format!("Repo {} not found", session.repo_id))?;

        let wt_path = self.worktree_path(&repo.name, branch_name);
        git::create_worktree(&repo.path, &wt_path, branch_name, &session.base_branch)
            .map_err(|e| format!("Failed to create worktree: {}", e))?;
        let _ = tmux::new_window(&session.name, branch_name, Some(&wt_path));

        let worktree = db::add_worktree(
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
        );

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

    /// Remove git worktree and delete DB record.
    pub fn cleanup_worktree(&self, worktree_id: i64) -> Result<(), String> {
        let wt = self
            .get_worktree_by_id(worktree_id)
            .ok_or_else(|| format!("Worktree {} not found", worktree_id))?;

        let repo = self
            .get_repo_by_id(wt.repo_id)
            .ok_or_else(|| format!("Repo {} not found", wt.repo_id))?;

        // Try to remove the git worktree; if it fails (e.g. directory already gone),
        // still delete the DB record
        if let Err(e) = git::remove_worktree(&repo.path, &wt.path) {
            // If the directory doesn't exist, just clean up the DB record
            if std::path::Path::new(&wt.path).exists() {
                return Err(format!("Failed to remove worktree: {}", e));
            }
        }
        db::delete_worktree(&self.conn, worktree_id);

        Ok(())
    }

    /// Return DB sessions enriched with live tmux state, plus unmanaged live sessions.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let db_sessions = db::get_sessions(&self.conn);
        let live_list = tmux::list_sessions();
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
        for ts in &live_list {
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
        if !home_dev.is_dir() {
            return;
        }
        let entries = match std::fs::read_dir(home_dev) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() || entry.file_name() == "worktrees" {
                continue;
            }
            if path.join(".git").exists() {
                let path_str = path.to_string_lossy().to_string();
                if !known_repos.contains_key(&path_str) {
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
                let repo_path_candidate = home_dev.join(&repo_dir_name).to_string_lossy().to_string();

                if !known_repos.contains_key(&repo_path_candidate) {
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

                let repo = &known_repos[&repo_path_candidate];
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
                let _repo_name = match components.next() {
                    Some(c) => c.as_os_str().to_string_lossy().to_string(),
                    None => continue,
                };
                let _branch_dir = match components.next() {
                    Some(c) => c.as_os_str().to_string_lossy().to_string(),
                    None => continue,
                };

                // The worktree path is worktrees_root/<repo>/<branch>
                let wt_path = worktrees_root
                    .join(&_repo_name)
                    .join(&_branch_dir)
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
                    // Find the repo for this worktree
                    let repos_dir = self.repos_dir();
                    let repo_path_candidate =
                        repos_dir.join(&_repo_name).to_string_lossy().to_string();
                    let repo = match known_repos.get(&repo_path_candidate) {
                        Some(r) => r,
                        None => continue,
                    };

                    db::add_worktree(
                        &self.conn,
                        &Worktree {
                            id: None,
                            repo_id: repo.id.unwrap(),
                            path: wt_path,
                            branch: _branch_dir,
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
                    },
                );
            }
        }
    }

    /// First-run adoption: scan repos and worktrees dirs, scan live tmux sessions,
    /// and populate the DB with discovered state.
    pub fn scan_existing(&self) {
        let home_dev = self.repos_dir();
        let worktrees_root = self.worktrees_dir();
        let mut known_repos: HashMap<String, Repo> = db::get_repos(&self.conn)
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();
        let mut known_worktree_paths: HashSet<String> = db::get_worktrees(&self.conn)
            .into_iter()
            .map(|wt| wt.path)
            .collect();

        self.scan_repos(&home_dev, &mut known_repos);
        self.scan_worktrees(&home_dev, &worktrees_root, &mut known_repos, &mut known_worktree_paths);
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
}
