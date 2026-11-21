//! Bounded text decoding and paging for text attachments and transcripts.
//!
//! Bytes are read under a cap ([`TEXT_READ_LIMIT`] for reads,
//! [`EXCERPT_READ_LIMIT`] for eager excerpts), decoded strictly, sanitized
//! and cut into pages small enough for one tool result. Sanitizing removes
//! what could make text render differently from what it says: control
//! characters and bidirectional overrides.
use super::{body::FetchedBody, cache::Weighted};
use std::fmt;

/// Bytes fetched for a text view.
pub const TEXT_READ_LIMIT: u64 = 1 << 20;
/// Bytes fetched for an eager excerpt.
pub const EXCERPT_READ_LIMIT: u64 = 256 * 1024;
/// The longest page, in UTF-8 bytes.
pub const PAGE_BYTES: usize = 16 * 1024;
/// The longest eager excerpt, in UTF-8 bytes.
pub const EXCERPT_BYTES: usize = 4 * 1024;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];
/// The longest incomplete UTF-8 sequence a truncated body can end with.
const MAX_INCOMPLETE_TAIL: usize = 3;

/// Decoded, sanitized text in pages of at most [`PAGE_BYTES`].
///
/// There is always at least one page, so page 1 of an empty file is an
/// empty page rather than an error.
#[derive(Clone, PartialEq, Eq)]
pub struct TextDocument {
    pub pages: Vec<String>,
    /// The body was cut at its read limit, so more text exists upstream.
    pub truncated: bool,
}

/// Content must never reach a log line, so only the sizes are shown.
impl fmt::Debug for TextDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextDocument")
            .field("pages", &self.pages.len())
            .field("bytes", &self.weight())
            .field("truncated", &self.truncated)
            .finish()
    }
}

impl Weighted for TextDocument {
    fn weight(&self) -> u64 {
        self.pages.iter().map(|page| page.len() as u64).sum()
    }
}

impl TextDocument {
    /// Page `text`, already normalized, into a document.
    pub fn from_normalized(text: &str, truncated: bool) -> Self {
        Self {
            pages: paginate(text, PAGE_BYTES),
            truncated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// Not UTF-8 or BOM-marked UTF-16.
    NotText,
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not_text")
    }
}

impl std::error::Error for TextError {}

/// Decode `body` as UTF-8 (with or without a BOM) or BOM-marked UTF-16,
/// sanitize it and page it.
///
/// UTF-8 is strict: an invalid sequence means the body is not text. Only a
/// body cut at its read limit may end in an incomplete sequence, which is
/// dropped. Unpaired UTF-16 surrogates become U+FFFD.
pub fn decode(body: &FetchedBody) -> Result<TextDocument, TextError> {
    let bytes = body.bytes.as_ref();
    let text = if let Some(rest) = bytes.strip_prefix(UTF16_LE_BOM) {
        decode_utf16(rest, body.truncated, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(UTF16_BE_BOM) {
        decode_utf16(rest, body.truncated, u16::from_be_bytes)
    } else {
        let rest = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
        decode_utf8(rest, body.truncated)?.to_owned()
    };
    Ok(TextDocument::from_normalized(
        &normalize(&text),
        body.truncated,
    ))
}

fn decode_utf8(bytes: &[u8], truncated: bool) -> Result<&str, TextError> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error)
            if truncated
                && error.error_len().is_none()
                && bytes.len() - error.valid_up_to() <= MAX_INCOMPLETE_TAIL =>
        {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| TextError::NotText)
        }
        Err(_) => Err(TextError::NotText),
    }
}

fn decode_utf16(bytes: &[u8], truncated: bool, unit: fn([u8; 2]) -> u16) -> String {
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| unit([pair[0], pair[1]]))
        .collect();
    // A cut-off body may end inside a code unit or between the halves of a
    // surrogate pair; neither is damage in the text itself.
    if truncated
        && units
            .last()
            .is_some_and(|last| (0xD800..0xDC00).contains(last))
    {
        units.pop();
    }
    let mut text: String = char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if bytes.len() % 2 == 1 && !truncated {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

/// Line endings become `\n`; control characters other than `\n` and `\t`
/// (C0, DEL and C1) and bidirectional controls are removed.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(character),
            _ if character.is_control() || is_bidi_control(character) => {}
            _ => out.push(character),
        }
    }
    out
}

/// Embeddings, overrides, isolates and directional marks: they reorder how
/// text displays without changing what it contains.
fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
    )
}

/// The first `max` bytes of the document, cut on a char boundary, and
/// whether more text exists beyond them.
pub fn excerpt(doc: &TextDocument, max: usize) -> (String, bool) {
    let mut out = String::new();
    let mut more = doc.truncated;
    for page in &doc.pages {
        let room = max - out.len();
        if page.len() <= room {
            out.push_str(page);
            continue;
        }
        out.push_str(&page[..floor_char_boundary(page, room)]);
        more = true;
        break;
    }
    (out, more)
}

/// `text` in pieces of at most `size` bytes, each ending on a char boundary.
fn paginate(text: &str, size: usize) -> Vec<String> {
    let mut pages = Vec::with_capacity(text.len() / size + 1);
    let mut rest = text;
    while rest.len() > size {
        let mut cut = floor_char_boundary(rest, size);
        if cut == 0 {
            // A size below one character still makes progress.
            cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        pages.push(rest[..cut].to_owned());
        rest = &rest[cut..];
    }
    if !rest.is_empty() || pages.is_empty() {
        pages.push(rest.to_owned());
    }
    pages
}

/// The largest char boundary of `text` at or below `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    (0..=index)
        .rev()
        .find(|&candidate| text.is_char_boundary(candidate))
        .unwrap_or(0)
}
