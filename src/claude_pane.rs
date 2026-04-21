// trellis/src/claude_pane.rs

use regex::Regex;
use std::fs;
use std::path::Path;

pub fn get_session_id(pane_pid: &str) -> Option<String> {
    if pane_pid.is_empty() {
        return None;
    }
    let pid_file = Path::new("/tmp/claude-sessions").join(format!("pid-{}", pane_pid));
    fs::read_to_string(pid_file).ok().map(|s| s.trim().to_string())
}

pub fn get_first_user_message(session_id: &str) -> Option<String> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    if !projects_dir.exists() {
        return None;
    }
    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for entry in entries {
        // Skip unreadable entries (matches Python's behavior of iterating past errors)
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let jsonl = entry.path().join(format!("{}.jsonl", session_id));
        if jsonl.exists() {
            return first_user_message_from_jsonl(&jsonl);
        }
    }
    None
}

fn first_user_message_from_jsonl(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        // Skip lines that fail to parse (matches Python's try/except JSONDecodeError: continue)
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let is_user = entry
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "user")
            .unwrap_or(false);
        if !is_user {
            continue;
        }
        let text = entry
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

pub fn summarize_message(message: &str, max_words: usize) -> String {
    let first_line = message.lines().next().unwrap_or("").trim().trim_start_matches('#').trim();
    let words: Vec<&str> = first_line.split_whitespace().take(max_words).collect();
    let name: String = words
        .iter()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| ".,!?:;\"'()[]{}".contains(c))
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
    if name.is_empty() {
        "claude".to_string()
    } else {
        name
    }
}

pub fn classify_pane(pane_text: &str) -> &'static str {
    let lines: Vec<&str> = pane_text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: &[&str] = if lines.len() > 10 {
        &lines[lines.len() - 10..]
    } else {
        &lines
    };
    if tail.is_empty() {
        return "idle";
    }
    let tail_text: String = tail.join("\n");

    // Use LazyLock to avoid recompiling regexes on every call (this is a hot path —
    // called for every expanded window row on every render cycle)
    use std::sync::LazyLock;
    static PROMPTING_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"❯\s+1\.").unwrap());
    static WORKING_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^\x00-\x7f]\s+.*…").unwrap());

    // Permission dialog: ❯ followed by numbered choice + "Esc to cancel"
    if PROMPTING_RE.is_match(&tail_text) && tail_text.contains("Esc to cancel") {
        return "prompting";
    }

    // Active spinner: non-ASCII char followed by text containing …
    // Matches both short spinners ("✻ Envisioning…") and long ones
    // ("✳ Transition to implementation planning… (3m 51s · ↓ 406 tokens)")
    if WORKING_RE.is_match(&tail_text) {
        return "working";
    }

    "idle"
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
    fn summarize_empty() {
        assert_eq!(summarize_message("", 4), "claude");
    }

    #[test]
    fn summarize_strips_heading() {
        assert_eq!(summarize_message("## Fix the bug", 4), "fix-the-bug");
    }

    #[test]
    fn classify_idle_empty() {
        assert_eq!(classify_pane(""), "idle");
    }

    #[test]
    fn classify_working() {
        assert_eq!(classify_pane("✻ Envisioning…"), "working");
    }

    #[test]
    fn classify_prompting() {
        let text = "some output\n❯ 1. Yes\n  2. No\nEsc to cancel";
        assert_eq!(classify_pane(text), "prompting");
    }

    #[test]
    fn classify_idle_normal() {
        assert_eq!(classify_pane("❯ some command\noutput"), "idle");
    }

    #[test]
    fn classify_working_long_spinner() {
        // Multi-word spinner line with timing info (thinking/planning state)
        let text = "✳ Transition to implementation planning… (3m 51s · ↓ 406 tokens)\n  ⎿  ✔ Explore project context\n     ◼ Transition to implementation planning";
        assert_eq!(classify_pane(text), "working");
    }

    #[test]
    fn classify_idle_past_tense_spinner() {
        // Idle: spinner shows past tense with duration, no …
        assert_eq!(classify_pane("✻ Brewed for 31s"), "idle");
        assert_eq!(classify_pane("✻ Baked for 2m 36s"), "idle");
        assert_eq!(classify_pane("✻ Cogitated for 43s"), "idle");
        assert_eq!(classify_pane("✻ Cooked for 3m 17s"), "idle");
    }
}
