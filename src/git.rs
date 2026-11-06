// trellis/src/git.rs

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

fn run(args: &[&str], cwd: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output().expect("failed to execute git")
}

/// How many directory levels below a repos dir are searched for repos.
pub const REPO_SCAN_DEPTH: usize = 2;

/// Git repos under `root`, descending into non-repo directories up to
/// `max_depth` levels. Hidden dirs, `worktrees` and `skip` are not searched.
pub fn find_repos(root: &Path, skip: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_repos(root, skip, max_depth, &mut found);
    found
}

fn collect_repos(dir: &Path, skip: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name();
        if !path.is_dir()
            || path == skip
            || name == "worktrees"
            || name.to_string_lossy().starts_with('.')
        {
            continue;
        }
        if path.join(".git").exists() {
            found.push(path);
        } else {
            collect_repos(&path, skip, depth - 1, found);
        }
    }
}

pub fn detect_default_branch(repo_path: &str) -> Result<String, GitError> {
    let output = run(
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
        Some(repo_path),
    );
    if output.status.success() {
        let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(branch) = refname.rsplit('/').next() {
            return Ok(branch.to_string());
        }
    }
    let output = run(&["branch", "--format=%(refname:short)"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to list branches in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let branches: Vec<&str> = stdout.trim().lines().collect();
    for candidate in &["main", "master"] {
        if branches.contains(candidate) {
            return Ok(candidate.to_string());
        }
    }
    Err(GitError(format!(
        "Could not determine default branch in '{}'",
        repo_path
    )))
}

pub fn list_branches(repo_path: &str) -> Result<Vec<String>, GitError> {
    let output = run(&["branch", "--format=%(refname:short)"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to list branches in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

#[derive(Debug)]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
}

pub fn list_worktrees(repo_path: &str) -> Result<Vec<GitWorktree>, GitError> {
    let output = run(&["worktree", "list", "--porcelain"], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to list worktrees in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(GitWorktree {
                path: path.to_string(),
                branch: None,
                commit: None,
            });
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            if let Some(ref mut wt) = current {
                wt.commit = Some(head.to_string());
            }
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if let Some(ref mut wt) = current {
                wt.branch = Some(
                    branch_ref
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch_ref)
                        .to_string(),
                );
            }
        } else if line == "detached" {
            if let Some(ref mut wt) = current {
                wt.branch = Some("(detached)".to_string());
            }
        }
    }
    if let Some(wt) = current {
        worktrees.push(wt);
    }
    Ok(worktrees)
}

pub fn create_worktree(
    repo_path: &str,
    worktree_path: &str,
    branch: &str,
    base_branch: &str,
) -> Result<(), GitError> {
    // Check if path is already in use
    let existing = list_worktrees(repo_path)?;
    for wt in &existing {
        if wt.path == worktree_path {
            return Err(GitError(format!(
                "Worktree path '{}' is already in use",
                worktree_path
            )));
        }
    }
    let output = run(
        &["worktree", "add", "--no-track", "-b", branch, worktree_path, base_branch],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to create worktree at '{}' (branch '{}' from '{}'): {}",
            worktree_path,
            branch,
            base_branch,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn remove_worktree(repo_path: &str, worktree_path: &str) -> Result<(), GitError> {
    let output = run(&["worktree", "remove", "--force", worktree_path], Some(repo_path));
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to remove worktree '{}': {}",
            worktree_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

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

pub fn get_pr_branch(repo_path: &str, pr_number: i64) -> Result<String, GitError> {
    let pr_str = pr_number.to_string();
    let output = Command::new("gh")
        .args(["pr", "view", &pr_str, "--json", "headRefName", "--jq", ".headRefName"])
        .current_dir(repo_path)
        .output()
        .expect("failed to execute gh");
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to get PR #{}: {}",
            pr_number,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err(GitError(format!("PR #{} has no head branch", pr_number)));
    }
    Ok(branch)
}

pub fn fetch_origin(repo_path: &str) -> Result<(), GitError> {
    let fetch = run(&["fetch", "origin"], Some(repo_path));
    if !fetch.status.success() {
        let stderr = String::from_utf8_lossy(&fetch.stderr).trim().to_string();
        return Err(GitError(format!("git fetch failed: {}", stderr)));
    }
    Ok(())
}

/// Fast-forward local `branch` to `origin/<branch>` (call after `fetch_origin`)
/// without ever merging into some other branch. If `branch` is checked out
/// somewhere, fast-forward it there; otherwise move the ref directly.
pub fn fast_forward_branch(repo_path: &str, branch: &str) -> Result<(), GitError> {
    let checked_out_at = list_worktrees(repo_path)?
        .into_iter()
        .find(|wt| wt.branch.as_deref() == Some(branch))
        .map(|wt| wt.path);
    let output = match checked_out_at {
        Some(path) => run(&["merge", "--ff-only", &format!("origin/{}", branch)], Some(&path)),
        None => run(&["fetch", "origin", &format!("{0}:{0}", branch)], Some(repo_path)),
    };
    if !output.status.success() {
        return Err(GitError(format!(
            "could not fast-forward '{}': {}",
            branch,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn fetch_branch(repo_path: &str, branch: &str) {
    run(&["fetch", "origin", branch], Some(repo_path));
}

pub fn is_branch_merged(repo_path: &str, branch: &str, into: &str) -> Result<bool, GitError> {
    let output = run(
        &["branch", "--merged", into, "--format=%(refname:short)"],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to check merged branches in '{}': {}",
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let merged: Vec<&str> = stdout.trim().lines().collect();
    Ok(merged.contains(&branch))
}

pub fn has_remote_branch(repo_path: &str, branch: &str) -> Result<bool, GitError> {
    let output = run(
        &["ls-remote", "--heads", "origin", branch],
        Some(repo_path),
    );
    if !output.status.success() {
        return Err(GitError(format!(
            "Failed to query remote for branch '{}' in '{}': {}",
            branch,
            repo_path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

#[cfg(test)]
pub(crate) mod tests {
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
        // macOS tempdirs live under /var which is symlinked to /private/var.
        // git resolves paths through the symlink, so canonicalize before comparing.
        let repo = tmp.path().canonicalize().unwrap().join("repo");
        let wt = tmp.path().canonicalize().unwrap().join("wt");
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
        let repo = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        // Create branch separately, then attach a worktree without -b.
        Command::new("git").args(["branch", "feat-existing"]).current_dir(&repo).output().unwrap();
        let wt = tmp.path().canonicalize().unwrap().join("wt");

        add_worktree_existing_branch(repo.to_str().unwrap(), wt.to_str().unwrap(), "feat-existing").unwrap();

        let listed = list_worktrees(repo.to_str().unwrap()).unwrap();
        assert!(listed.iter().any(|w| w.branch.as_deref() == Some("feat-existing")));
    }

    fn make_repos(root: &std::path::Path, dirs: &[&str]) {
        for d in dirs {
            fs::create_dir_all(root.join(d).join(".git")).unwrap();
        }
    }

    #[test]
    fn find_repos_descends_into_container_dirs_but_not_into_repos() {
        let tmp = tempfile::tempdir().unwrap();
        make_repos(tmp.path(), &["top", "group/nested", "top/inner"]);
        let mut found = find_repos(tmp.path(), std::path::Path::new("/nonexistent"), REPO_SCAN_DEPTH);
        found.sort();
        assert_eq!(found, vec![tmp.path().join("group/nested"), tmp.path().join("top")]);
    }

    #[test]
    fn find_repos_respects_depth_skip_and_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        make_repos(tmp.path(), &["a/b/c", "skipme/x", ".hidden/y", "worktrees/z"]);
        let skip = tmp.path().join("skipme");
        assert!(find_repos(tmp.path(), &skip, REPO_SCAN_DEPTH).is_empty());
        assert_eq!(find_repos(tmp.path(), &skip, 3), vec![tmp.path().join("a/b/c")]);
    }

    pub(crate) fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo on `main` whose `origin` is a bare clone that has since moved
    /// one commit ahead. Returns (repo, origin tip sha).
    pub(crate) fn repo_behind_origin(root: &std::path::Path) -> (std::path::PathBuf, String) {
        let root = root.canonicalize().unwrap();
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let origin = root.join("origin.git");
        git_out(&root, &["clone", "-q", "--bare", repo.to_str().unwrap(), origin.to_str().unwrap()]);
        git_out(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);

        let pusher = root.join("pusher");
        git_out(&root, &["clone", "-q", origin.to_str().unwrap(), pusher.to_str().unwrap()]);
        git_out(&pusher, &["config", "user.email", "t@t"]);
        git_out(&pusher, &["config", "user.name", "t"]);
        git_out(&pusher, &["commit", "--allow-empty", "-qm", "ahead"]);
        git_out(&pusher, &["push", "-q", "origin", "main"]);
        let tip = git_out(&pusher, &["rev-parse", "HEAD"]);
        fetch_origin(repo.to_str().unwrap()).unwrap();
        (repo, tip)
    }

    #[test]
    fn fast_forward_moves_main_without_touching_the_checked_out_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = repo_behind_origin(tmp.path());
        git_out(&repo, &["checkout", "-q", "-b", "work"]);
        let work_before = git_out(&repo, &["rev-parse", "work"]);

        fast_forward_branch(repo.to_str().unwrap(), "main").unwrap();

        assert_eq!(git_out(&repo, &["rev-parse", "main"]), tip);
        assert_eq!(git_out(&repo, &["rev-parse", "work"]), work_before);
        assert_eq!(git_out(&repo, &["branch", "--show-current"]), "work");
    }

    #[test]
    fn fast_forward_updates_main_when_it_is_the_checked_out_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = repo_behind_origin(tmp.path());
        fast_forward_branch(repo.to_str().unwrap(), "main").unwrap();
        assert_eq!(git_out(&repo, &["rev-parse", "HEAD"]), tip);
    }

    #[test]
    fn fast_forward_updates_main_checked_out_in_another_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, tip) = repo_behind_origin(tmp.path());
        git_out(&repo, &["checkout", "-q", "-b", "work"]);
        let wt = tmp.path().canonicalize().unwrap().join("main-wt");
        git_out(&repo, &["worktree", "add", "-q", wt.to_str().unwrap(), "main"]);

        fast_forward_branch(repo.to_str().unwrap(), "main").unwrap();
        assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), tip);
    }

    #[test]
    fn fast_forward_refuses_diverged_main_and_leaves_it_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _) = repo_behind_origin(tmp.path());
        git_out(&repo, &["checkout", "-q", "-b", "work"]);
        git_out(&repo, &["commit", "--allow-empty", "-qm", "local-only"]);
        git_out(&repo, &["branch", "-f", "main", "work"]);
        let before = git_out(&repo, &["rev-parse", "main"]);

        assert!(fast_forward_branch(repo.to_str().unwrap(), "main").is_err());
        assert_eq!(git_out(&repo, &["rev-parse", "main"]), before);
    }

    #[test]
    fn fetch_origin_fails_without_a_reachable_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        git_out(&repo, &["remote", "add", "origin", "/nonexistent/origin.git"]);
        assert!(fetch_origin(repo.to_str().unwrap()).is_err());
    }
}
