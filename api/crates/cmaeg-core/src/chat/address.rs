//! Leading bot addresses and native platform mention syntax in chat text.
//!
//! The turn renderer strips these and the routing resolver skips them before
//! reading consumer addressing, so both share the matching rules below and
//! can never disagree about what counts as an address.

use crate::{activity::SourceKind, channel::Platform};

/// The Lark/Feishu placeholder that replaces a mention, followed by digits.
const MENTION_KEY_PREFIX: &str = "@_user_";

/// Remove every word-bounded, case-insensitive `@<bot_username>` from `text`, or, for
/// Lark and Feishu relay activities, the leading `@_user_N` mention keys. Native
/// Discord removes only exact leading receiving-bot mentions. The caller chooses
/// which bot username applies to transports that address bots by username.
pub fn strip_address(text: &str, source: &SourceKind, bot_username: Option<&str>) -> String {
    if uses_mention_keys(source) {
        return strip_mention_keys(text);
    }
    if let SourceKind::Discord { bot_id } = source {
        return text[discord_addresses_end(text, bot_id)..]
            .trim_end()
            .to_owned();
    }
    let Some(username) = bot_username else {
        return text.to_owned();
    };
    let mut output = String::new();
    let mut cursor = 0;
    for (index, _) in text.match_indices('@') {
        if index < cursor {
            continue;
        }
        let Some(end) = bot_address_end(text, index, username) else {
            continue;
        };
        output.push_str(&text[cursor..index]);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output.trim().to_owned()
}

/// Lark and Feishu replace each mention with a placeholder key such as `@_user_1`;
/// the keys that open the message address it and carry no meaning for the agent.
pub fn strip_mention_keys(text: &str) -> String {
    text[mention_keys_end(text)..].trim_end().to_owned()
}

/// The byte offset where the leading run of whitespace and addresses ends:
/// the bot addresses `strip_address` removes or, for Lark and Feishu relay
/// activities, the mention keys it removes. Only the leading run counts; an
/// address later in the text is not part of it.
pub fn leading_address_end(text: &str, source: &SourceKind, bot_username: Option<&str>) -> usize {
    if uses_mention_keys(source) {
        return mention_keys_end(text);
    }
    if let SourceKind::Discord { bot_id } = source {
        return discord_addresses_end(text, bot_id);
    }
    let mut offset = whitespace_end(text, 0);
    let Some(username) = bot_username else {
        return offset;
    };
    while let Some(end) = bot_address_end(text, offset, username) {
        offset = whitespace_end(text, end);
    }
    offset
}

/// The length of a word-bounded, case-insensitive `@<bot_username>` at the
/// very start of `text`, as in a `/to@bot` command suffix.
pub fn bot_address_len(text: &str, bot_username: &str) -> Option<usize> {
    bot_address_end(text, 0, bot_username)
}

/// Lark and Feishu activities address the bot through mention keys, which
/// replace its username entirely.
fn uses_mention_keys(source: &SourceKind) -> bool {
    matches!(
        source,
        SourceKind::NyxidRelay {
            platform: Platform::Lark | Platform::Feishu,
            ..
        }
    )
}

/// Native Discord identifies its receiving bot by ID, never a display name.
/// Only its exact leading user mentions are addresses; role/channel mentions,
/// other users and mention-shaped text later in the message stay untouched.
fn discord_addresses_end(text: &str, bot_id: &str) -> usize {
    let mut offset = whitespace_end(text, 0);
    if !crate::channel::discord::valid_id(bot_id) {
        return offset;
    }
    while let Some(mention) = text[offset..].strip_prefix("<@") {
        let mention = mention.strip_prefix('!').unwrap_or(mention);
        let Some(tail) = mention
            .strip_prefix(bot_id)
            .and_then(|rest| rest.strip_prefix('>'))
        else {
            break;
        };
        offset = whitespace_end(text, text.len() - tail.len());
    }
    offset
}

/// Where `@<username>` starting at `index` ends, when it is there: matched
/// case-insensitively and word-bounded, so `@botx`, `x@bot`, `@@bot` and
/// `@bot@` are not addresses.
fn bot_address_end(text: &str, index: usize, username: &str) -> Option<usize> {
    let end = index + 1 + username.len();
    let address = text.get(index..end)?;
    if !address
        .strip_prefix('@')
        .is_some_and(|name| name.eq_ignore_ascii_case(username))
    {
        return None;
    }
    let boundary = |value: char| value.is_alphanumeric() || value == '_' || value == '@';
    if text[..index].chars().next_back().is_some_and(boundary)
        || text[end..].chars().next().is_some_and(boundary)
    {
        return None;
    }
    Some(end)
}

/// Where the leading whitespace and mention keys end. A key needs at least
/// one digit and must be followed by whitespace or the end of the text.
fn mention_keys_end(text: &str) -> usize {
    let mut offset = whitespace_end(text, 0);
    while let Some(after) = text[offset..].strip_prefix(MENTION_KEY_PREFIX) {
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
        offset = whitespace_end(text, text.len() - tail.len());
    }
    offset
}

/// The offset after the whitespace that starts at `from`.
fn whitespace_end(text: &str, from: usize) -> usize {
    text.len() - text[from..].trim_start().len()
}
