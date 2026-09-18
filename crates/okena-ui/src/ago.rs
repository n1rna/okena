//! How long ago something happened, to the precision a person reads it at.

/// How long ago `then` was, both in Unix millis.
pub fn format_ago(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then) / 1_000;
    match secs {
        0..60 => "just now".into(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// The current time in Unix millis, for [`format_ago`]'s `now`.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::format_ago;

    #[test]
    fn ages_read_at_a_human_precision() {
        assert_eq!(format_ago(0, 30_000), "just now");
        assert_eq!(format_ago(0, 5 * 60_000), "5 min ago");
        assert_eq!(format_ago(0, 3 * 3_600_000), "3 h ago");
        assert_eq!(format_ago(0, 2 * 86_400_000), "2 d ago");
        // A clock that went backwards is "just now", not a panic.
        assert_eq!(format_ago(10_000, 0), "just now");
    }
}
