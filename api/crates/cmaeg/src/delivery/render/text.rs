//! Port of bridge/outbox/text.rs, preserving escape atoms and UTF-16 limits.

/// One scalar as it is sent: markup characters escaped, mentions defused,
/// angle brackets replaced, and control and bidi characters dropped.
fn atom(scalar: char) -> String {
    match scalar {
        '@' => "@\u{200b}".into(),
        '<' => "‹".into(),
        '>' => "›".into(),
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => String::new(),
        c if c.is_control() && !matches!(c, '\n' | '\t') => String::new(),
        c if "\\_*`[]~|".contains(c) => format!("\\{c}"),
        c => c.to_string(),
    }
}

/// `text` with every scalar escaped exactly as message text is.
pub(super) fn escape(text: &str) -> String {
    text.chars().map(atom).collect()
}

/// Split `text` into parts of at most `limit` UTF-16 units, preferring
/// paragraph boundaries. The first part leaves `reserved` units free for a
/// prefix; later parts keep the full limit. The first part's own limit never
/// drops below the widest atom, two units.
pub(super) fn chunks_reserving(text: &str, limit: usize, reserved: usize) -> Vec<String> {
    let first_limit = limit.saturating_sub(reserved).max(2);
    let mut result = Vec::new();
    let mut part = String::new();
    let mut units = 0;
    let mut paragraph = None;
    for scalar in text.chars() {
        let atom = atom(scalar);
        let size = atom.encode_utf16().count();
        let current = if result.is_empty() {
            first_limit
        } else {
            limit
        };
        if units + size > current {
            if let Some(boundary) = paragraph.take() {
                let tail = part.split_off(boundary);
                result.push(std::mem::replace(&mut part, tail));
                units = part.encode_utf16().count();
            } else {
                result.push(std::mem::take(&mut part));
                units = 0;
            }
        }
        part.push_str(&atom);
        units += size;
        if part.ends_with("\n\n") {
            paragraph = Some(part.len());
        }
    }
    if !part.is_empty() {
        result.push(part);
    }
    result
}
