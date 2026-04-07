pub fn fuzzy_match(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();

    if query.is_empty() {
        return Some(0);
    }

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut last_match: i32 = -2;

    for (ti, &ch) in text.iter().enumerate() {
        if qi < query.len() && ch == query[qi] {
            let ti_i32 = ti as i32;
            if ti_i32 == last_match + 1 {
                score -= 1;
            } else {
                score += ti_i32;
            }
            last_match = ti_i32;
            qi += 1;
        }
    }

    if qi < query.len() {
        None
    } else {
        Some(score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
    }

    #[test]
    fn exact_match() {
        assert_eq!(fuzzy_match("abc", "abc"), Some(-2));
    }

    #[test]
    fn no_match() {
        assert_eq!(fuzzy_match("xyz", "abc"), None);
    }

    #[test]
    fn partial_match_fails() {
        assert_eq!(fuzzy_match("abcd", "abc"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_match("ABC", "abc").is_some());
    }

    #[test]
    fn gap_penalized_by_position() {
        assert_eq!(fuzzy_match("ac", "abc"), Some(2));
    }

    #[test]
    fn better_match_scores_lower() {
        let close = fuzzy_match("ab", "ab___").unwrap();
        let far = fuzzy_match("ab", "a___b").unwrap();
        assert!(close < far);
    }
}
