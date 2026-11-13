//! Stable first-line encoding of the gateway-attested identity projection.
use super::{ActivityIdentity, BindingRouteError};

pub(super) fn line(activity: &ActivityIdentity) -> Result<String, BindingRouteError> {
    use BindingRouteError::SenderUnsupported;
    let source = &activity.source;
    let platform = match source.kind.as_str() {
        "native_lark" => match source.region.as_deref() {
            Some(region @ ("lark" | "feishu")) => Some(region),
            _ => return Err(SenderUnsupported),
        },
        "telegram" | "nyxid_relay" => Some(
            source
                .platform
                .as_deref()
                .filter(|p| !p.is_empty() && p.len() <= 32)
                .ok_or(SenderUnsupported)?,
        ),
        "whatsapp" => Some("whatsapp"),
        "discord" => Some("discord"),
        "github" => Some("github"),
        "custom" => None,
        _ => return Err(SenderUnsupported),
    };
    if activity.actor.id.len() > 2048
        || !matches!(
            activity.actor.kind.as_str(),
            "human" | "bot" | "system" | "unknown"
        )
        || !matches!(
            activity.conversation.kind.as_str(),
            "private" | "group" | "channel" | "thread" | "custom"
        )
        || (activity.actor.kind == "human" && activity.actor.id.is_empty())
    {
        return Err(SenderUnsupported);
    }
    #[derive(serde::Serialize)]
    struct Line<'a> {
        v: u8,
        source_kind: &'a str,
        platform: Option<&'a str>,
        actor_id: &'a str,
        actor_kind: &'a str,
        conversation_kind: &'a str,
        human_sender: bool,
    }
    let line = serde_json::to_string(&Line {
        v: 1,
        source_kind: &source.kind,
        platform,
        actor_id: &activity.actor.id,
        actor_kind: &activity.actor.kind,
        conversation_kind: &activity.conversation.kind,
        human_sender: activity.actor.kind == "human",
    })
    .map_err(|_| SenderUnsupported)?;
    let mut escaped = String::with_capacity(line.len());
    for character in line.chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            use std::fmt::Write;
            write!(escaped, "\\u{:04x}", u32::from(character)).map_err(|_| SenderUnsupported)?;
        } else {
            escaped.push(character);
        }
    }
    Ok(format!("cma_verified_sender: {escaped}\n"))
}
