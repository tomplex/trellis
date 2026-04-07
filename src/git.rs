// torchard-rs/src/git.rs

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
        &["worktree", "add", "-b", branch, worktree_path, base_branch],
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

pub fn fetch_and_pull(repo_path: &str, branch: &str) {
    run(&["fetch", "origin"], Some(repo_path));
    run(&["pull", "origin", branch], Some(repo_path));
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
