//! Bounded document classification from complete bytes, never a supplied label.
#[path = "outbound_document/compound.rs"]
mod compound;
#[path = "outbound_document/package.rs"]
mod package;
#[path = "outbound_document/xml.rs"]
mod xml;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Document {
    Text,
    Pdf,
    Word,
    Excel,
    PowerPoint,
    WordXml,
    ExcelXml,
    PowerPointXml,
}
impl Document {
    pub(crate) fn mime(self) -> &'static str {
        match self {
            Self::Text => "text/plain",
            Self::Pdf => "application/pdf",
            Self::Word => "application/msword",
            Self::Excel => "application/vnd.ms-excel",
            Self::PowerPoint => "application/vnd.ms-powerpoint",
            Self::WordXml => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            Self::ExcelXml => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::PowerPointXml => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
        }
    }
    pub(crate) fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Text => &["txt"],
            Self::Pdf => &["pdf"],
            Self::Word => &["doc"],
            Self::Excel => &["xls"],
            Self::PowerPoint => &["ppt"],
            Self::WordXml => &["docx"],
            Self::ExcelXml => &["xlsx"],
            Self::PowerPointXml => &["pptx"],
        }
    }
}

pub(crate) fn inspect(bytes: &[u8]) -> Option<Document> {
    if bytes.is_empty() || bytes.len() as u64 > crate::media::outbound::MAX_OUTBOUND_BYTES {
        return None;
    }
    if bytes.starts_with(b"%PDF-") {
        let version = bytes.get(5..8)?;
        return ((version[0] == b'1' && (b'0'..=b'7').contains(&version[2]) || version == b"2.0")
            && version[1] == b'.'
            && matches!(bytes.get(8), Some(b'\r' | b'\n'))
            && bytes.trim_ascii_end().ends_with(b"%%EOF"))
        .then_some(Document::Pdf);
    }
    if bytes.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        return compound::inspect(bytes);
    }
    if bytes.starts_with(b"PK") {
        return package::inspect(bytes);
    }
    // A recognized binary type cannot become a text document through relabeling.
    if infer::get(&bytes[..bytes.len().min(64)]).is_some() {
        return None;
    }
    text(bytes).then_some(Document::Text)
}

fn text(bytes: &[u8]) -> bool {
    let permitted = |c: char| !c.is_control() || matches!(c, '\t' | '\n' | '\r' | '\u{c}');
    if bytes.starts_with(b"\xff\xfe") || bytes.starts_with(b"\xfe\xff") {
        if !bytes.len().is_multiple_of(2) {
            return false;
        }
        let little = bytes[0] == 0xff;
        return char::decode_utf16(bytes[2..].chunks_exact(2).map(|b| {
            if little {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        }))
        .all(|c| c.is_ok_and(permitted));
    }
    std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
        .is_ok_and(|s| s.chars().all(permitted))
}

#[cfg(test)]
#[path = "outbound_document/tests.rs"]
pub(crate) mod tests;
