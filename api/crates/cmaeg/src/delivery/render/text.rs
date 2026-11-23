//! Shared sanitization; chunking keeps each escape atom within UTF-16 limits.
pub(super) fn escaped(text: &str) -> String {
    atoms(text).collect()
}

fn atoms(text: &str) -> impl Iterator<Item = String> + '_ {
    text.chars().map(|scalar| match scalar {
        '@' => "@\u{200b}".into(),
        '<' => "‹".into(),
        '>' => "›".into(),
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => String::new(),
        c if c.is_control() && !matches!(c, '\n' | '\t') => String::new(),
        c if "\\_*`[]~|".contains(c) => format!("\\{c}"),
        c => c.to_string(),
    })
}

pub(super) fn chunks(text: &str, limit: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut part = String::new();
    let mut units = 0;
    let mut paragraph = None;
    for atom in atoms(text) {
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
