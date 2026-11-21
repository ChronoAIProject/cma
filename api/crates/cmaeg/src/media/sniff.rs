//! Content type from the bytes themselves.
//!
//! The sniffed type always wins over a declared MIME type or file name: both
//! are sender-controlled, so a `.txt` that is a PNG is a PNG. Recognition uses
//! the maintained `infer` matcher table rather than a hand-rolled one.

/// The broad media family a fetched body belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Image,
    Audio,
    Video,
    Text,
    Other,
}

/// The sniffed family and MIME type of a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sniffed {
    pub family: Family,
    pub mime: &'static str,
}

/// Classify `head`, the first bytes of a body.
///
/// Image, audio and video recognized by `infer` keep its family; everything
/// else it recognizes (PDF, Office files, archives and the rest) is `Other`
/// with its MIME type. Unrecognized bytes are text when they carry a UTF-8 or
/// UTF-16 byte order mark, or are UTF-8 without a NUL byte.
pub fn sniff(head: &[u8]) -> Sniffed {
    if let Some(kind) = infer::get(head) {
        let family = match kind.matcher_type() {
            infer::MatcherType::Image => Family::Image,
            infer::MatcherType::Audio => Family::Audio,
            infer::MatcherType::Video => Family::Video,
            _ => Family::Other,
        };
        return Sniffed {
            family,
            mime: kind.mime_type(),
        };
    }
    if is_text(head) {
        return Sniffed {
            family: Family::Text,
            mime: "text/plain",
        };
    }
    Sniffed {
        family: Family::Other,
        mime: "application/octet-stream",
    }
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];
/// The longest incomplete UTF-8 sequence a cut-off head can end with.
const MAX_INCOMPLETE_TAIL: usize = 3;

fn is_text(head: &[u8]) -> bool {
    if head.starts_with(UTF8_BOM)
        || head.starts_with(UTF16_LE_BOM)
        || head.starts_with(UTF16_BE_BOM)
    {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // A head is a prefix, so a multibyte character may be cut off at its
        // end; only an incomplete final sequence is tolerated.
        Err(error) => {
            error.error_len().is_none() && head.len() - error.valid_up_to() <= MAX_INCOMPLETE_TAIL
        }
    }
}

const TEXT_MIME_TYPES: [&str; 6] = [
    "application/json",
    "application/xml",
    "application/yaml",
    "application/x-yaml",
    "application/javascript",
    "application/x-sh",
];
const TEXT_EXTENSIONS: [&str; 32] = [
    "txt", "md", "markdown", "csv", "tsv", "json", "jsonl", "yaml", "yml", "xml", "html", "htm",
    "log", "ini", "toml", "conf", "cfg", "py", "rs", "ts", "js", "go", "java", "c", "h", "cpp",
    "sh", "sql", "rb", "kt", "swift", "php",
];

/// Whether the *declared* MIME type or file name suggests text.
///
/// This is only a hint for deciding eager work before anything is fetched;
/// once bytes exist, [`sniff`] decides.
pub fn text_like_hint(mime: Option<&str>, filename: Option<&str>) -> bool {
    let mime = mime.map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    });
    if mime
        .as_deref()
        .is_some_and(|mime| mime.starts_with("text/") || TEXT_MIME_TYPES.contains(&mime))
    {
        return true;
    }
    filename
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .is_some_and(|extension| TEXT_EXTENSIONS.contains(&extension.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_incomplete_tail_longer_than_one_character_is_not_text() {
        assert!(is_text("é".as_bytes()));
        assert!(is_text(&"abc€".as_bytes()[..5]));
        // An invalid byte in the middle is never tolerated.
        assert!(!is_text(&[b'a', 0xC3, b'b']));
        // A lone continuation byte is invalid, not incomplete.
        assert!(!is_text(&[b'a', 0x80]));
    }
}
