//! Shared sanitization; chunking keeps each escape atom within UTF-16 limits.

/// How Markdown punctuation reaches the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextStyle {
    /// Escape Markdown punctuation for destinations that parse the legacy
    /// relay dialect.
    Escaped,
    /// Keep punctuation literal for destinations that show plain text (Lark)
    /// or parse their own Markdown (Discord), so links stay intact.
    Literal,
}

pub(super) fn escaped(text: &str) -> String {
    atoms(text, TextStyle::Escaped).collect()
}

pub(super) fn literal(text: &str) -> String {
    atoms(text, TextStyle::Literal).collect()
}

/// Mention neutralization and control/bidi filtering apply in every style.
/// The literal style keeps `@` inside an `http(s)://` destination so links
/// such as profile URLs survive, except where it would spell a broadcast
/// mention.
fn atoms(text: &str, style: TextStyle) -> impl Iterator<Item = String> + '_ {
    let mut url_end = 0;
    text.char_indices().map(move |(at, scalar)| {
        if style == TextStyle::Literal && at >= url_end {
            url_end = at + url_len(&text[at..]);
        }
        match scalar {
            '@' if at < url_end && !broadcast(&text[at + 1..]) => "@".into(),
            '@' => "@\u{200b}".into(),
            '<' => "‹".into(),
            '>' => "›".into(),
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => String::new(),
            c if c.is_control() && !matches!(c, '\n' | '\t') => String::new(),
            c if style == TextStyle::Escaped && "\\_*`[]~|".contains(c) => format!("\\{c}"),
            c => c.to_string(),
        }
    })
}

/// Byte length of the `http(s)://` destination starting here, or zero.
fn url_len(rest: &str) -> usize {
    let scheme = ["https://", "http://"].into_iter().any(|scheme| {
        rest.get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    });
    if !scheme {
        return 0;
    }
    rest.find(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"'))
        .unwrap_or(rest.len())
}

fn broadcast(after_at: &str) -> bool {
    after_at.starts_with("everyone") || after_at.starts_with("here")
}

/// Lark and Feishu show relayed and native text as plain text, and Discord
/// parses its own Markdown; escaping Markdown punctuation breaks their links.
pub fn literal_platform(platform: cmaeg_core::channel::Platform) -> bool {
    use cmaeg_core::channel::Platform;
    matches!(
        platform,
        Platform::Lark | Platform::Feishu | Platform::Discord
    )
}

pub(super) fn chunks(text: &str, limit: usize, style: TextStyle) -> Vec<String> {
    let mut result = Vec::new();
    let mut part = String::new();
    let mut units = 0;
    let mut paragraph = None;
    for atom in atoms(text, style) {
        let size = atom.encode_utf16().count();
        if units + size > limit {
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
