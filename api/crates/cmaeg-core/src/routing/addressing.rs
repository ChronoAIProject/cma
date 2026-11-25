//! Leading chat addressing over the raw message text.
//!
//! ```text
//! lead        = *( WSP / bot-address / mention-key )
//! bot-address = "@" <bot_username of input.chat_source>   ; case-insensitive, word-bounded
//!             / "<@" [ "!" ] <native Discord bot_id> ">" ; exact receiving bot only
//! mention-key = "@_user_" 1*DIGIT                          ; Lark/Feishu activities only
//! command     = lead "/to" [ "@" <bot_username> ] 1*WSP target *( "," target ) ( 1*WSP rest / end )
//! mentions    = lead "@" target [ "," / ":" ] *( 1*WSP "@" target [ "," / ":" ] ) ( 1*WSP rest / end )
//! target      = consumer name (case-insensitive) or consumer id
//! ```
//!
//! `lead` is read by `cmaeg_core::chat`, the same rules the turn renderer
//! strips, so the two never disagree about a bot address. Only a leading
//! run addresses consumers: a mention later in the text is ordinary text.
//! Every scan is linear in the text, which activity validation bounds.
use super::{
    roster::{token_payload, Roster, Targets},
    RouteError, RouteInput,
};
use crate::{
    activity::{ActivityKind, RouteResolution},
    channel::{ChannelDefinition, Source},
    chat,
    ids::ConsumerId,
};

/// The command whose arguments carry consumer addressing.
const COMMAND: &str = "to";
const COMMAND_PREFIX: &str = "/to";

/// Consumers a message addressed explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressed {
    /// In order, without duplicates.
    pub targets: Vec<ConsumerId>,
    /// `mention` or `command`.
    pub resolution: RouteResolution,
    /// The byte offset in the raw text (in `args` for a `to` command) where
    /// the unaddressed remainder begins, after `lead` and the addressing.
    pub addressing_bytes: u16,
}

/// The consumers a message or `to` command addresses, or `None` when no
/// consumer addressing leads it (or the channel has no routing). A `/to`
/// target that is unknown or not mentionable is refused; a leading `@token`
/// that names no consumer ends the mentions, since it may address a human.
pub fn parse(
    def: &ChannelDefinition,
    input: &RouteInput<'_>,
) -> Result<Option<Addressed>, RouteError> {
    let Some(roster) = Roster::new(def) else {
        return Ok(None);
    };
    match recognize(&roster, input.activity, input.chat_source) {
        Some(form) => form.resolve(&roster),
        None => Ok(None),
    }
}

/// A legacy launcher can honor only an explicit command to the default consumer.
/// Ordinary text and disabled command addressing retain their legacy semantics.
pub fn validate_legacy_command(
    def: &ChannelDefinition,
    activity: &crate::activity::Activity,
    chat_source: Option<&Source>,
) -> Result<(), RouteError> {
    let Some(roster) = Roster::new(def) else {
        return Ok(());
    };
    let Some(form) = recognize(&roster, activity, chat_source)
        .filter(|form| form.resolution() == RouteResolution::Command)
    else {
        return Ok(());
    };
    let Some(addressed) = form.resolve(&roster)? else {
        return Ok(());
    };
    if matches!(addressed.targets.as_slice(), [target] if target.is_default()) {
        Ok(())
    } else {
        Err(RouteError::KillSwitchClosed)
    }
}

/// Addressing recognized by its syntax, before any target is resolved.
pub(super) struct Form<'t> {
    resolution: RouteResolution,
    /// The text the offsets index: the message text, or a command's `args`.
    text: &'t str,
    /// Where the first target begins: after `/to` and its whitespace, or at
    /// the first mention's `@`.
    start: usize,
}

/// The addressing form the activity uses, when the channel enables it.
pub(super) fn recognize<'t>(
    roster: &Roster<'_>,
    activity: &'t crate::activity::Activity,
    chat_source: Option<&Source>,
) -> Option<Form<'t>> {
    let enabled = &roster.policy.addressing;
    match &activity.kind {
        ActivityKind::Message { text, .. } => {
            let username = bot_username(chat_source);
            let lead = chat::leading_address_end(text, &activity.source, username);
            let addressed = &text[lead..];
            if enabled.command && addressed.starts_with(COMMAND_PREFIX) {
                let start = command_targets_start(text, lead, username)?;
                return Some(Form {
                    resolution: RouteResolution::Command,
                    text,
                    start,
                });
            }
            (enabled.mentions && addressed.starts_with('@')).then_some(Form {
                resolution: RouteResolution::Mention,
                text,
                start: lead,
            })
        }
        ActivityKind::Command { name, args } if enabled.command && name == COMMAND => Some(Form {
            resolution: RouteResolution::Command,
            text: args,
            start: whitespace_end(args, 0),
        }),
        _ => None,
    }
}

impl Form<'_> {
    /// `mention` or `command`, known before any target is resolved.
    pub(super) fn resolution(&self) -> RouteResolution {
        self.resolution
    }

    /// Resolve the recognized form's targets against the roster.
    pub(super) fn resolve(&self, roster: &Roster<'_>) -> Result<Option<Addressed>, RouteError> {
        let mut targets = Targets::new(roster.max_targets());
        let rest = if self.resolution == RouteResolution::Command {
            command_targets(roster, self.text, self.start, &mut targets)?
        } else {
            mention_targets(roster, self.text, self.start, &mut targets)?
        };
        let Some(rest) = rest else {
            return Ok(None);
        };
        // Activity text is at most 32 KiB, so an offset always fits; anything
        // longer is malformed rather than silently cut short.
        let addressing_bytes = u16::try_from(rest).map_err(|_| RouteError::InvalidTargets)?;
        Ok(Some(Addressed {
            targets: targets.into_ids(),
            resolution: self.resolution,
            addressing_bytes,
        }))
    }
}

/// Where a `/to` command's targets begin: after `/to`, an optional
/// `@<bot_username>` suffix and at least one whitespace character.
fn command_targets_start(text: &str, lead: usize, username: Option<&str>) -> Option<usize> {
    let mut offset = lead + COMMAND_PREFIX.len();
    if let Some(suffix) = username.and_then(|name| chat::bot_address_len(&text[offset..], name)) {
        offset += suffix;
    }
    let start = whitespace_end(text, offset);
    (start > offset).then_some(start)
}

/// Read `target *( "," target )` at `start`; every target must name a
/// mentionable consumer. Returns where the remainder begins, or `None` when
/// no target follows the command.
fn command_targets(
    roster: &Roster<'_>,
    text: &str,
    start: usize,
    targets: &mut Targets,
) -> Result<Option<usize>, RouteError> {
    let end = word_end(text, start);
    if end == start {
        return Ok(None);
    }
    for token in text[start..end].split(',') {
        let member = roster
            .named(token)
            .ok_or_else(|| RouteError::TargetUnknown(token_payload(token)))?;
        if !member.view.mentionable {
            return Err(RouteError::TargetNotMentionable(member.name.to_string()));
        }
        targets.push(member.id())?;
    }
    Ok(Some(whitespace_end(text, end)))
}

/// Read the leading run of `@target` mentions at `start`. The run stops at
/// the first token naming no consumer; that token begins the remainder.
/// Returns `None` when the first token names no consumer.
fn mention_targets(
    roster: &Roster<'_>,
    text: &str,
    start: usize,
    targets: &mut Targets,
) -> Result<Option<usize>, RouteError> {
    let mut offset = start;
    let mut found = false;
    while text[offset..].starts_with('@') {
        let end = word_end(text, offset);
        let word = &text[offset + 1..end];
        let token = word.strip_suffix([',', ':']).unwrap_or(word);
        let Some(member) = roster.named(token) else {
            break;
        };
        if !member.view.mentionable {
            return Err(RouteError::TargetNotMentionable(member.name.to_string()));
        }
        targets.push(member.id())?;
        found = true;
        offset = whitespace_end(text, end);
    }
    Ok(found.then_some(offset))
}

/// The bot username of the source the activity arrived on.
fn bot_username(source: Option<&Source>) -> Option<&str> {
    match source? {
        Source::Telegram { bot_username, .. } | Source::NyxidRelay { bot_username, .. } => {
            bot_username.as_deref()
        }
        Source::NativeLark(_)
        | Source::Custom { .. }
        | Source::Discord { .. }
        | Source::Whatsapp { .. }
        | Source::Github { .. } => None,
    }
}

/// The offset of the first whitespace character at or after `from`.
fn word_end(text: &str, from: usize) -> usize {
    text[from..]
        .find(char::is_whitespace)
        .map_or(text.len(), |index| from + index)
}

/// The offset after the whitespace that starts at `from`.
fn whitespace_end(text: &str, from: usize) -> usize {
    text.len() - text[from..].trim_start().len()
}
