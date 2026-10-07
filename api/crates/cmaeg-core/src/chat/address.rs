//! Leading bot addresses and Lark/Feishu mention keys in chat text.

use crate::{activity::SourceKind, channel::Platform};

/// Remove every word-bounded, case-insensitive `@<bot_username>` from `text`, or, for
/// Lark and Feishu relay activities, the leading `@_user_N` mention keys. Unchanged
/// behaviour moved from the gateway renderer; the caller chooses which bot username applies.
pub fn strip_address(text: &str, source: &SourceKind, bot_username: Option<&str>) -> String {
    if let SourceKind::NyxidRelay {
        platform: Platform::Lark | Platform::Feishu,
        ..
    } = source
    {
        return strip_mention_keys(text);
    }
    let Some(username) = bot_username else {
        return text.to_owned();
    };
    let target = format!("@{username}");
    let mut output = String::new();
    let mut cursor = 0;
    for (index, _) in text.match_indices('@') {
        let end = index + target.len();
        if index < cursor
            || !text
                .get(index..end)
                .is_some_and(|part| part.eq_ignore_ascii_case(&target))
        {
            continue;
        }
        let boundary = |value: char| value.is_alphanumeric() || value == '_' || value == '@';
        if text[..index].chars().next_back().is_some_and(boundary)
            || text[end..].chars().next().is_some_and(boundary)
        {
            continue;
        }
        output.push_str(&text[cursor..index]);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output.trim().to_owned()
}
/// Lark and Feishu replace each mention with a placeholder key such as `@_user_1`;
/// the keys that open the message address it and carry no meaning for the agent.
pub fn strip_mention_keys(text: &str) -> String {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix("@_user_") {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let tail = &after[digits..];
        if digits == 0
            || tail
                .chars()
                .next()
                .is_some_and(|next| !next.is_whitespace())
        {
            break;
        }
        rest = tail.trim_start();
    }
    rest.trim_end().to_owned()
}
