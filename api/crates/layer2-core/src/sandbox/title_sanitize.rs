pub(super) fn sanitized_title(value: &str, limit: usize) -> Option<String> {
    let first_line = value.split(['\r', '\n']).next().unwrap_or_default();
    let filtered = first_line
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let mut candidate = filtered.trim().to_owned();

    loop {
        let previous = candidate.clone();
        candidate = strip_markdown_prefix(&candidate).trim().to_owned();
        candidate = strip_markdown_suffix(&candidate).trim().to_owned();
        candidate = strip_balanced_wrapper(&candidate)
            .unwrap_or(&candidate)
            .trim()
            .trim_matches(is_decorative_symbol)
            .trim()
            .to_owned();
        if candidate == previous {
            break;
        }
    }

    if candidate.is_empty() || candidate.chars().all(is_ornament) {
        return None;
    }
    let bounded = candidate.chars().take(limit).collect::<String>();
    let bounded = bounded.trim().trim_matches(is_decorative_symbol).trim();
    (!bounded.is_empty() && !bounded.chars().all(is_ornament)).then(|| bounded.to_owned())
}

fn strip_markdown_prefix(value: &str) -> &str {
    let value = value.trim();
    let hashes = value
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if (1..=6).contains(&hashes) {
        let rest = &value[hashes..];
        if rest.starts_with(char::is_whitespace) {
            return rest.trim_start();
        }
    }
    for marker in [">", "-", "+", "*", "\u{2022}", "\u{25aa}"] {
        if let Some(rest) = value.strip_prefix(marker) {
            if rest.starts_with(char::is_whitespace) {
                return rest.trim_start();
            }
        }
    }
    value
}

fn strip_markdown_suffix(value: &str) -> &str {
    let value = value.trim();
    let hashes = value
        .chars()
        .rev()
        .take_while(|character| *character == '#')
        .count();
    if hashes > 0 {
        let end = value.len() - hashes;
        if value[..end].ends_with(char::is_whitespace) {
            return value[..end].trim_end();
        }
    }
    value
}

fn strip_balanced_wrapper(value: &str) -> Option<&str> {
    let mut characters = value.char_indices();
    let (_, first) = characters.next()?;
    let (last_index, last) = characters.next_back()?;
    if last_index < first.len_utf8() || !is_wrapper_pair(first, last) {
        return None;
    }
    Some(value[first.len_utf8()..last_index].trim())
}

fn is_wrapper_pair(first: char, last: char) -> bool {
    matches!(
        (first, last),
        ('"', '"')
            | ('\'', '\'')
            | ('`', '`')
            | ('*', '*')
            | ('_', '_')
            | ('~', '~')
            | ('=', '=')
            | ('|', '|')
            | ('[', ']')
            | ('(', ')')
            | ('{', '}')
            | ('\u{201c}', '\u{201d}')
            | ('\u{2018}', '\u{2019}')
            | ('\u{00ab}', '\u{00bb}')
            | ('\u{2039}', '\u{203a}')
            | ('\u{3010}', '\u{3011}')
            | ('\u{300c}', '\u{300d}')
            | ('\u{300e}', '\u{300f}')
            | ('\u{2014}', '\u{2014}')
    )
}

fn is_decorative_symbol(character: char) -> bool {
    matches!(
        character,
        '\u{2022}'
            | '\u{25aa}'
            | '\u{25ab}'
            | '\u{25c6}'
            | '\u{25c7}'
            | '\u{2605}'
            | '\u{2606}'
            | '\u{2726}'
            | '\u{2727}'
            | '\u{2728}'
    )
}

fn is_ornament(character: char) -> bool {
    character.is_whitespace()
        || is_decorative_symbol(character)
        || matches!(
            character,
            '#' | '*' | '_' | '~' | '=' | '|' | '>' | '`' | '"' | '\''
        )
}
