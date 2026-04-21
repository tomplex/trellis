// trellis/src/tmux.rs

use std::collections::HashMap;
use std::process::Command;

#[derive(Debug)]
pub struct TmuxError(pub String);

impl std::fmt::Display for TmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TmuxError {}

pub fn sanitize_session_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c == '.' || c == ':' { '-' } else { c })
        .collect();
    let trimmed = sanitized.trim_matches(|c: char| c == ' ' || c == '-');
    if trimmed.is_empty() {
        "new-session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new("tmux")
        .args(args)
        .output()
        .expect("failed to execute tmux")
}

#[derive(Debug, Clone)]
pub struct TmuxSession {
    pub name: String,
    pub windows: i64,
    pub attached: bool,
}

pub fn list_sessions() -> Vec<TmuxSession> {
    let output = run(&[
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_windows}\t#{session_attached}",
    ]);
    if !output.status.success() {
        return vec![];
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                return None;
            }
            Some(TmuxSession {
                name: parts[0].to_string(),
                windows: parts[1].parse().unwrap_or(0),
                attached: parts[2] == "1",
            })
        })
        .collect()
}

pub fn session_exists(name: &str) -> bool {
    run(&["has-session", "-t", name]).status.success()
}

pub fn new_session(name: &str, start_dir: &str) -> Result<(), TmuxError> {
    if session_exists(name) {
        return Err(TmuxError(format!("Session '{}' already exists", name)));
    }
    let output = run(&["new-session", "-d", "-s", name, "-c", start_dir]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to create session '{}': {}",
            name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn switch_client(target: &str) -> Result<(), TmuxError> {
    let output = run(&["switch-client", "-t", target]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to switch to '{}': {}",
            target,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn attach_session(target: &str) -> Result<(), TmuxError> {
    let status = Command::new("tmux")
        .args(["attach-session", "-t", target])
        .status()
        .map_err(|e| TmuxError(format!("Failed to attach to '{}': {}", target, e)))?;
    if !status.success() {
        return Err(TmuxError(format!("Failed to attach to '{}'", target)));
    }
    Ok(())
}

pub fn inside_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

pub fn new_window(session: &str, name: &str, start_dir: Option<&str>) -> Result<(), TmuxError> {
    let mut args = vec!["new-window", "-t", session, "-n", name];
    if let Some(dir) = start_dir {
        args.extend_from_slice(&["-c", dir]);
    }
    let output = run(&args);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to create window '{}' in '{}': {}",
            name,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn select_window(session: &str, index: i64) -> Result<(), TmuxError> {
    let target = format!("{}:{}", session, index);
    let output = run(&["select-window", "-t", &target]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to select window {} in '{}': {}",
            index,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn rename_window(session: &str, index: i64, new_name: &str) -> Result<(), TmuxError> {
    let target = format!("{}:{}", session, index);
    let output = run(&["rename-window", "-t", &target, new_name]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to rename window {} in '{}': {}",
            index,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn rename_session(old_name: &str, new_name: &str) -> Result<(), TmuxError> {
    let output = run(&["rename-session", "-t", old_name, new_name]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to rename session '{}' to '{}': {}",
            old_name,
            new_name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct TmuxWindow {
    pub index: i64,
    pub name: String,
    pub path: String,
    pub command: String,
    pub pane_pid: String,
}

pub fn list_all_windows() -> HashMap<String, Vec<TmuxWindow>> {
    let output = run(&[
        "list-windows",
        "-a",
        "-F",
        "#{session_name}\t#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}",
    ]);
    if !output.status.success() {
        return HashMap::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut by_session: HashMap<String, Vec<TmuxWindow>> = HashMap::new();
    for line in stdout.trim().lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let session = parts[0].to_string();
        let win = TmuxWindow {
            index: parts[1].parse().unwrap_or(0),
            name: parts[2].to_string(),
            path: parts[3].to_string(),
            command: parts.get(4).unwrap_or(&"").to_string(),
            pane_pid: parts.get(5).unwrap_or(&"").to_string(),
        };
        by_session.entry(session).or_default().push(win);
    }
    by_session
}

pub fn list_windows(session: &str) -> Vec<TmuxWindow> {
    let output = run(&[
        "list-windows",
        "-t",
        session,
        "-F",
        "#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}",
    ]);
    if !output.status.success() {
        return vec![];
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                return None;
            }
            Some(TmuxWindow {
                index: parts[0].parse().unwrap_or(0),
                name: parts[1].to_string(),
                path: parts[2].to_string(),
                command: parts.get(3).unwrap_or(&"").to_string(),
                pane_pid: parts.get(4).unwrap_or(&"").to_string(),
            })
        })
        .collect()
}

pub fn kill_window(session: &str, index: i64) -> Result<(), TmuxError> {
    let target = format!("{}:{}", session, index);
    let output = run(&["kill-window", "-t", &target]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to kill window {} in '{}': {}",
            index,
            session,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn send_keys(target: &str, keys: &[&str]) {
    let mut args = vec!["send-keys", "-t", target];
    args.extend_from_slice(keys);
    run(&args);
}

pub fn capture_pane(target: &str, lines: i64) -> String {
    let lines_arg = format!("-{}", lines);
    let output = run(&["capture-pane", "-t", target, "-p", "-J", "-S", &lines_arg]);
    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::new()
    }
}


pub fn kill_session(name: &str) -> Result<(), TmuxError> {
    let output = run(&["kill-session", "-t", name]);
    if !output.status.success() {
        return Err(TmuxError(format!(
            "Failed to kill session '{}': {}",
            name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_dots_and_colons() {
        assert_eq!(sanitize_session_name("foo.bar:baz"), "foo-bar-baz");
    }

    #[test]
    fn sanitize_trims() {
        assert_eq!(sanitize_session_name(" -hello- "), "hello");
    }

    #[test]
    fn sanitize_empty_fallback() {
        assert_eq!(sanitize_session_name("..."), "new-session");
    }
}
