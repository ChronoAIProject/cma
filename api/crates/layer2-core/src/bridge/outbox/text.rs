use super::OutboxError;
use crate::bridge::BridgePlatform;

/// Keep escape atoms and UTF-8 scalars intact. UTF-16 counts conservatively
/// bound Telegram/Discord; Lark uses the same lower limit for text-only output.
pub(super) fn chunks(platform: &BridgePlatform, text: &str) -> Result<Vec<String>, OutboxError> {
    if text.len() > 1_048_576 {
        return Err(OutboxError::Bounds);
    }
    let limit = match platform {
        BridgePlatform::Telegram | BridgePlatform::TelegramNew => 4000,
        _ => 1900,
    };
    let mut result = Vec::new();
    let mut part = String::new();
    let mut units = 0;
    for scalar in text.chars() {
        // NyxID Telegram forces legacy Markdown and Discord omits
        // allowed_mentions. Escape markup and break mentions before transport.
        let atom = match scalar {
            '@' => "@\u{200b}".into(),
            '<' => "‹".into(),
            '>' => "›".into(),
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => String::new(),
            c if c.is_control() && !matches!(c, '\n' | '\t') => String::new(),
            c if matches!(
                platform,
                BridgePlatform::Telegram | BridgePlatform::TelegramNew | BridgePlatform::Discord
            ) && "\\_*`[]~|".contains(c) =>
            {
                format!("\\{c}")
            }
            c => c.to_string(),
        };
        let size = atom.encode_utf16().count();
        if units + size > limit {
            result.push(std::mem::take(&mut part));
            units = 0;
        }
        part.push_str(&atom);
        units += size;
    }
    if !part.is_empty() {
        result.push(part);
    }
    Ok(result)
}
