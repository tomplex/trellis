// trellis/src/tui/path_complete.rs

use std::path::PathBuf;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use super::theme;

/// zsh-style directory completion for path inputs: Tab extends to the common
/// prefix, then cycles through the matches; Shift-Tab cycles back.
#[derive(Default)]
pub struct PathCompleter {
    base: String,
    matches: Vec<String>,
    selected: Option<usize>,
}

impl PathCompleter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn complete(&mut self, input: &mut String, cursor: &mut usize, backward: bool) {
        if let Some(sel) = self.selected {
            let n = self.matches.len();
            let next = if backward { (sel + n - 1) % n } else { (sel + 1) % n };
            self.select(next, input, cursor);
            return;
        }
        if backward {
            return;
        }

        if input == "~" {
            *input = "~/".into();
            *cursor = input.len();
            return;
        }
        let split = input.rfind('/').map_or(0, |i| i + 1);
        let (base, prefix) = input.split_at(split);
        let (base, prefix) = (base.to_string(), prefix.to_string());
        let matches = list_dirs(&base, &prefix);

        match matches.len() {
            0 => self.reset(),
            1 => {
                *input = format!("{base}{}/", matches[0]);
                *cursor = input.len();
                self.reset();
            }
            _ => {
                let common = common_prefix(&matches);
                self.base = base;
                self.matches = matches;
                if common.len() > prefix.len() {
                    *input = format!("{}{common}", self.base);
                    *cursor = input.len();
                } else {
                    self.select(0, input, cursor);
                }
            }
        }
    }

    fn select(&mut self, idx: usize, input: &mut String, cursor: &mut usize) {
        self.selected = Some(idx);
        *input = format!("{}{}", self.base, self.matches[idx]);
        *cursor = input.len();
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if self.matches.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self.matches.iter().map(|m| ListItem::new(m.as_str())).collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::ACCENT))
                    .style(Style::default().bg(theme::BG)),
            )
            .highlight_style(Style::default().bg(theme::CURSOR_BG).add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected(self.selected);
        f.render_stateful_widget(list, area, &mut state);
    }
}

fn expand_home(base: &str) -> PathBuf {
    match base.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None if base.is_empty() => PathBuf::from("."),
        None => PathBuf::from(base),
    }
}

/// Sorted names of directories in `base` starting with `prefix`; hidden ones
/// only when `prefix` itself starts with a dot.
fn list_dirs(base: &str, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(expand_home(base))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    names
}

fn common_prefix(names: &[String]) -> String {
    let mut common = names[0].clone();
    for n in &names[1..] {
        let len = common
            .chars()
            .zip(n.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum();
        common.truncate(len);
    }
    common
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dirs: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for d in dirs {
            std::fs::create_dir_all(root.path().join(d)).unwrap();
        }
        root
    }

    fn tab(c: &mut PathCompleter, input: &mut String, backward: bool) {
        let mut cursor = input.len();
        c.complete(input, &mut cursor, backward);
        assert_eq!(cursor, input.len());
    }

    #[test]
    fn single_match_completes_with_trailing_slash() {
        let root = fixture(&["sensible_apps/scorecard", "other"]);
        let mut input = format!("{}/sen", root.path().display());
        tab(&mut PathCompleter::default(), &mut input, false);
        assert_eq!(input, format!("{}/sensible_apps/", root.path().display()));
    }

    #[test]
    fn ambiguous_extends_to_common_prefix_then_cycles() {
        let root = fixture(&["score_a", "score_b", "other"]);
        let base = format!("{}/", root.path().display());
        let mut c = PathCompleter::default();
        let mut input = format!("{base}s");

        tab(&mut c, &mut input, false);
        assert_eq!(input, format!("{base}score_"));
        assert_eq!(c.selected, None);

        tab(&mut c, &mut input, false);
        assert_eq!(input, format!("{base}score_a"));
        tab(&mut c, &mut input, false);
        assert_eq!(input, format!("{base}score_b"));
        tab(&mut c, &mut input, false);
        assert_eq!(input, format!("{base}score_a"));
        tab(&mut c, &mut input, true);
        assert_eq!(input, format!("{base}score_b"));
    }

    #[test]
    fn reset_stops_cycling() {
        let root = fixture(&["aa", "ab"]);
        let mut c = PathCompleter::default();
        let mut input = format!("{}/a", root.path().display());
        tab(&mut c, &mut input, false);
        tab(&mut c, &mut input, false);
        assert!(c.selected.is_some());
        c.reset();
        assert!(c.selected.is_none() && c.matches.is_empty());
    }

    #[test]
    fn files_and_hidden_dirs_are_not_offered() {
        let root = fixture(&[".hidden", "visible"]);
        std::fs::write(root.path().join("visible_file"), "").unwrap();
        assert_eq!(list_dirs(&format!("{}/", root.path().display()), ""), vec!["visible"]);
        assert_eq!(list_dirs(&format!("{}/", root.path().display()), "."), vec![".hidden"]);
    }

    #[test]
    fn no_match_leaves_input_alone() {
        let root = fixture(&["aa"]);
        let mut input = format!("{}/zz", root.path().display());
        let before = input.clone();
        tab(&mut PathCompleter::default(), &mut input, false);
        assert_eq!(input, before);
    }
}
