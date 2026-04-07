// torchard-rs/src/conversation_index.rs

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

use crate::models::Conversation;

pub fn index_path() -> PathBuf {
    dirs::home_dir()
        .unwrap()
        .join(".claude")
        .join("conversation-index.md")
}

pub fn parse_index(path: Option<&Path>) -> Vec<Conversation> {
    let path = path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(index_path);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let header_re = Regex::new(r"^## (\d{4}-\d{2}-\d{2} \d{2}:\d{2}) \[([0-9a-f]+)\]").unwrap();
    let project_re = Regex::new(r"^- \*\*project\*\*: `(.+)`").unwrap();
    let branch_re = Regex::new(r"^- \*\*branch\*\*: `(.+)`").unwrap();
    let intent_re = Regex::new(r"^  - (.+)").unwrap();

    let mut entries: Vec<Conversation> = Vec::new();
    let mut current: Option<Conversation> = None;
    let mut in_intent = false;

    for line in content.lines() {
        if let Some(caps) = header_re.captures(line) {
            if let Some(conv) = current.take() {
                entries.push(conv);
            }
            current = Some(Conversation {
                date: caps[1].to_string(),
                session_id: caps[2].to_string(),
                project: String::new(),
                branch: String::new(),
                intents: Vec::new(),
            });
            in_intent = false;
            continue;
        }

        let cur = match current.as_mut() {
            Some(c) => c,
            None => continue,
        };

        if let Some(caps) = project_re.captures(line) {
            cur.project = caps[1].to_string();
            in_intent = false;
            continue;
        }

        if let Some(caps) = branch_re.captures(line) {
            cur.branch = caps[1].to_string();
            in_intent = false;
            continue;
        }

        if line.trim() == "- **intent**:" {
            in_intent = true;
            continue;
        }

        if in_intent {
            if let Some(caps) = intent_re.captures(line) {
                cur.intents.push(caps[1].to_string());
                continue;
            } else {
                in_intent = false;
            }
        }
    }

    if let Some(conv) = current {
        entries.push(conv);
    }

    entries.reverse();
    entries
}

pub fn resolve_session_id(short_id: &str, project_path: &str) -> String {
    let encoded = project_path.replace('/', "-");
    let projects_dir = dirs::home_dir()
        .unwrap()
        .join(".claude")
        .join("projects")
        .join(&encoded);
    if projects_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if stem.starts_with(short_id) {
                            return stem.to_string();
                        }
                    }
                }
            }
        }
    }
    short_id.to_string()
}

pub fn filter_by_paths(entries: &[Conversation], paths: &[String]) -> Vec<Conversation> {
    entries
        .iter()
        .filter(|e| paths.iter().any(|p| e.project.starts_with(p)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_index() -> String {
        // File is oldest-first (as written by the index tool); parse_index reverses to newest-first
        r#"## 2026-03-30 14:30 [def67890]
- **project**: `/home/user/dev/other`
- **branch**: `feature`
- **intent**:
  - Refactor auth module

## 2026-04-01 10:00 [abc12345]
- **project**: `/home/user/dev/myrepo`
- **branch**: `main`
- **intent**:
  - Fix the login bug
  - Add tests
"#
        .to_string()
    }

    #[test]
    fn parse_index_entries() {
        let tmp = std::env::temp_dir().join("torchard-test-index.md");
        let mut f = fs::File::create(&tmp).unwrap();
        f.write_all(sample_index().as_bytes()).unwrap();

        let entries = parse_index(Some(&tmp));
        assert_eq!(entries.len(), 2);

        // Newest first
        assert_eq!(entries[0].session_id, "abc12345");
        assert_eq!(entries[0].date, "2026-04-01 10:00");
        assert_eq!(entries[0].project, "/home/user/dev/myrepo");
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].intents.len(), 2);
        assert_eq!(entries[0].summary(), "Fix the login bug");

        assert_eq!(entries[1].session_id, "def67890");

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn parse_index_missing_file() {
        let entries = parse_index(Some(Path::new("/nonexistent/file.md")));
        assert!(entries.is_empty());
    }

    #[test]
    fn filter_by_paths_works() {
        let entries = vec![
            Conversation {
                date: "d".into(),
                session_id: "a".into(),
                project: "/home/user/dev/myrepo".into(),
                branch: "main".into(),
                intents: vec![],
            },
            Conversation {
                date: "d".into(),
                session_id: "b".into(),
                project: "/home/user/dev/other".into(),
                branch: "main".into(),
                intents: vec![],
            },
        ];
        let filtered = filter_by_paths(&entries, &["/home/user/dev/myrepo".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].session_id, "a");
    }
}
