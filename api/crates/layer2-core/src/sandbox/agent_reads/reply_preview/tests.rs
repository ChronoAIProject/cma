use crate::sandbox::normalize_reply_preview;

#[test]
fn preview_normalization_removes_controls_and_collapses_spacing() {
    assert_eq!(
        normalize_reply_preview(" \n Hello\r\n世界\t\u{0000}🦀\u{202e} "),
        Some(("Hello 世界 🦀".into(), false))
    );
    assert_eq!(normalize_reply_preview(" \n\t\u{0000}\u{202e}"), None);
}

#[test]
fn preview_truncation_counts_unicode_scalars_without_splitting_utf8() {
    for (count, truncated) in [(239, false), (240, false), (241, true)] {
        let (text, actual) = normalize_reply_preview(&"🦀".repeat(count)).unwrap();
        assert_eq!(text.chars().count(), count.min(240));
        assert_eq!(actual, truncated);
    }
    assert_eq!(
        normalize_reply_preview(&("x".repeat(240) + " \n")),
        Some(("x".repeat(240), false))
    );
}
