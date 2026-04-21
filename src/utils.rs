pub fn utc_now() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339).unwrap()
}

pub fn truncate_end(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_len - 1).collect();
    format!("{truncated}…")
}

pub fn truncate_start(text: &str, max_len: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_len {
        return text.to_string();
    }
    let skip = char_count - (max_len - 1);
    let truncated: String = text.chars().skip(skip).collect();
    format!("…{truncated}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_end_short() {
        assert_eq!(truncate_end("hello", 10), "hello");
    }

    #[test]
    fn truncate_end_exact() {
        assert_eq!(truncate_end("hello", 5), "hello");
    }

    #[test]
    fn truncate_end_long() {
        assert_eq!(truncate_end("hello world", 8), "hello w…");
    }

    #[test]
    fn truncate_start_short() {
        assert_eq!(truncate_start("hello", 10), "hello");
    }

    #[test]
    fn truncate_start_long() {
        assert_eq!(truncate_start("/very/long/path/here", 12), "…g/path/here");
    }
}
