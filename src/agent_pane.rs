// trellis/src/agent_pane.rs
//
// Agent panes announce themselves through a small status file instead of
// trellis scraping their screen. A harness extension (see `pi/trellis.ts`)
// writes `~/.local/share/trellis/panes/<pane>.json` keyed by its `$TMUX_PANE`
// (leading `%` dropped) and keeps it current across the agent's lifecycle:
//
//   { "agent": "pi", "pid": 12345, "state": "working", "title": "fix login" }
//
// `state` is one of `idle`, `working`, `prompting`. `title` is the first user
// message of the conversation and feeds the window auto-rename. The file is
// removed on a clean quit; a stale one (crash, pane reused) is rejected by the
// pid liveness check.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PaneStatus {
    pub agent: String,
    pub pid: u32,
    pub state: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub fn status_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/trellis/panes")
}

/// Read the status file for a tmux pane id (`%12`). `None` when the pane is
/// not an agent pane, the file is malformed, or the writer is no longer alive.
pub fn read_status(pane_id: &str) -> Option<PaneStatus> {
    let key = pane_id.strip_prefix('%').unwrap_or(pane_id);
    if key.is_empty() {
        return None;
    }
    let raw = fs::read(status_dir().join(format!("{}.json", key))).ok()?;
    let status: PaneStatus = serde_json::from_slice(&raw).ok()?;
    if !process_alive(status.pid) {
        return None;
    }
    Some(status)
}

fn process_alive(pid: u32) -> bool {
    // Signal 0 performs the permission/existence check without delivering.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Collapse a conversation's first message into a short tmux window name.
pub fn summarize_message(message: &str, max_words: usize) -> String {
    let first_line = message.lines().next().unwrap_or("").trim().trim_start_matches('#').trim();
    let words: Vec<&str> = first_line.split_whitespace().take(max_words).collect();
    let name: String = words
        .iter()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| ".,!?:;\"'`()[]{}".contains(c))
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if name.len() > 30 {
        if let Some(pos) = name[..30].rfind('-') {
            return name[..pos].to_string();
        }
        return name[..30].to_string();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_basic() {
        assert_eq!(summarize_message("Fix the login bug", 4), "fix-the-login-bug");
    }

    #[test]
    fn summarize_truncates_words() {
        assert_eq!(
            summarize_message("one two three four five six", 4),
            "one-two-three-four"
        );
    }

    #[test]
    fn summarize_strips_punctuation() {
        assert_eq!(summarize_message("Hello, world!", 4), "hello-world");
    }

    #[test]
    fn summarize_strips_backticks() {
        assert_eq!(summarize_message("run `echo ok` with bash", 4), "run-echo-ok-with");
    }

    #[test]
    fn summarize_empty() {
        assert_eq!(summarize_message("", 4), "");
    }

    #[test]
    fn summarize_strips_heading() {
        assert_eq!(summarize_message("## Fix the bug", 4), "fix-the-bug");
    }

    #[test]
    fn status_rejects_dead_writer() {
        let raw = r#"{"agent":"pi","pid":4194303,"state":"idle"}"#;
        let status: PaneStatus = serde_json::from_str(raw).unwrap();
        // PID_MAX on macOS is 99998, on Linux defaults to 4194304, so this pid
        // cannot be a live process on either.
        assert!(!process_alive(status.pid));
    }

    #[test]
    fn status_accepts_live_writer_and_optional_title() {
        let raw = format!(
            r#"{{"agent":"pi","pid":{},"state":"working"}}"#,
            std::process::id()
        );
        let status: PaneStatus = serde_json::from_str(&raw).unwrap();
        assert!(process_alive(status.pid));
        assert_eq!(status.state, "working");
        assert!(status.title.is_none());
    }
}
