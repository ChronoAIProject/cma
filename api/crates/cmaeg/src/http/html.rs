//! HTML text escaping shared by the gateway's server-rendered pages.

/// Escape `value` for HTML text and quoted attribute values.
pub(crate) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::escape;

    #[test]
    fn escapes_each_markup_character() {
        for (input, expected) in [
            ("&", "&amp;"),
            ("<", "&lt;"),
            (">", "&gt;"),
            ("\"", "&quot;"),
            ("'", "&#39;"),
        ] {
            assert_eq!(escape(input), expected);
        }
        assert_eq!(
            escape(r#"<a href="x">Tom & 'Jerry'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn escapes_existing_entities_again() {
        assert_eq!(escape("&amp;"), "&amp;amp;");
        assert_eq!(escape("&lt;script&gt;"), "&amp;lt;script&amp;gt;");
    }

    #[test]
    fn passes_plain_and_non_ascii_text_through() {
        assert_eq!(escape(""), "");
        assert_eq!(escape("plain text"), "plain text");
        assert_eq!(escape("Grüße, 你好 👋"), "Grüße, 你好 👋");
    }
}
