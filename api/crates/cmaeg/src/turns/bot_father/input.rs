use cmaeg_core::activity::ActivityKind;
use serde_json::{json, Value};

/// Only the explicit setup command, its login return, and its own callbacks
/// bypass the provider. The agent interprets all ordinary conversation,
/// including unqualified choice words, JSON and natural-language Bot requests.
pub(super) fn is_setup_activity(activity: &cmaeg_core::activity::Activity) -> bool {
    match &activity.kind {
        ActivityKind::Interaction(result) => is_setup_block(&result.block_id),
        _ => setup_text(activity).is_some(),
    }
}

fn setup_text(activity: &cmaeg_core::activity::Activity) -> Option<&str> {
    match &activity.kind {
        ActivityKind::Message { text, .. } => {
            let text = text.trim();
            if let Some(value) = text.strip_prefix("/setup") {
                return (value.is_empty() || value.starts_with(char::is_whitespace))
                    .then(|| value.trim());
            }
            if let Some(value) = text.strip_prefix("/start ") {
                let value = value.trim();
                return if value == "setup" {
                    Some("")
                } else if is_login_return(value) {
                    Some(value)
                } else {
                    None
                };
            }
            is_login_return(text).then_some(text)
        }
        ActivityKind::Command { name, args } if name == "setup" => Some(args.trim()),
        ActivityKind::Command { name, args } if name == "start" => {
            let args = args.trim();
            if args == "setup" {
                Some("")
            } else if is_login_return(args) {
                Some(args)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_login_return(text: &str) -> bool {
    text.strip_prefix("link_").is_some_and(|proof| {
        !proof.is_empty()
            && proof.len() <= 128
            && proof
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn is_setup_block(value: &str) -> bool {
    value.strip_prefix("bst_").is_some_and(|value| {
        value.split_once('.').is_some_and(|(id, revision)| {
            id.len() == 32
                && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !revision.is_empty()
                && revision.bytes().all(|byte| byte.is_ascii_digit())
                && revision.parse::<u64>().is_ok_and(|revision| revision > 0)
        })
    })
}

/// The gateway translates the documented text envelope, while CMA validates
/// profile fields, service authority and the current workflow step.
pub(super) fn request(activity: &cmaeg_core::activity::Activity) -> Result<(Value, Value), String> {
    let Some(text) = setup_text(activity) else {
        return Ok((json!({}), json!({})));
    };
    if !text.starts_with('{') && !text.starts_with("```") {
        return Ok((draft(activity), json!({})));
    }
    let invalid = || {
        "Send the complete JSON template from the current Customize step with `/setup ` before the JSON: {\"agent\": {...}}, {\"workspace\": {...}}, or {\"sandbox\": {...}, \"availability\": {...}}. Keep the outer object and use valid JSON; you can also reply `/setup back` or `/setup cancel`.".to_owned()
    };
    let text = if let Some(fenced) = text.strip_prefix("```json") {
        fenced
            .trim()
            .strip_suffix("```")
            .ok_or_else(invalid)?
            .trim()
    } else if let Some(fenced) = text.strip_prefix("```") {
        fenced
            .trim()
            .strip_suffix("```")
            .ok_or_else(invalid)?
            .trim()
    } else {
        text
    };
    let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    let object = value.as_object().ok_or_else(invalid)?;
    let profiles = ["agent", "workspace", "sandbox"];
    if profiles
        .iter()
        .filter(|key| object.contains_key(**key))
        .count()
        != 1
        || object.iter().any(|(key, content)| {
            !matches!(
                key.as_str(),
                "agent" | "workspace" | "sandbox" | "availability"
            ) || !content.is_object()
        })
        || (object.contains_key("availability") && !object.contains_key("sandbox"))
    {
        return Err(invalid());
    }
    Ok((json!({}), value))
}

pub(super) fn draft(activity: &cmaeg_core::activity::Activity) -> Value {
    let Some(trimmed) = setup_text(activity) else {
        return json!({});
    };
    if trimmed.is_empty()
        || trimmed == "/start"
        || trimmed.starts_with("link_")
        || trimmed.starts_with("/start link_")
        || setup_command(trimmed)
    {
        return json!({});
    }
    let lower = trimmed.to_ascii_lowercase();
    let name = if let Some(value) = trimmed.strip_prefix("name:") {
        Some(value.trim()).filter(|value| !value.is_empty())
    } else if let Some(start) = trimmed.find("叫 ") {
        let value = &trimmed[start + "叫 ".len()..];
        value
            .split(" 的 Bot")
            .next()
            .or_else(|| value.split("的 Bot").next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    } else if let Some(start) = lower.find("called ") {
        let value = &trimmed[start + "called ".len()..];
        value
            .split(['.', ',', ':'])
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let instructions = [
        "Agent instructions：",
        "Agent instructions:",
        "instructions:",
    ]
    .iter()
    .find_map(|marker| {
        trimmed
            .find(marker)
            .map(|index| &trimmed[index + marker.len()..])
    })
    .map(str::trim)
    .filter(|value| !value.is_empty());
    if name.is_none() && instructions.is_none() && trimmed.len() <= 80 {
        return json!({"name": trimmed});
    }
    json!({
        "name": name,
        "instructions": instructions,
    })
}

fn setup_command(value: &str) -> bool {
    let value = value
        .trim()
        .strip_prefix("/choice ")
        .or_else(|| value.trim().strip_prefix("choice "))
        .unwrap_or(value.trim());
    let lower = value.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "cancel"
            | "new"
            | "edit"
            | "refresh"
            | "register"
            | "done"
            | "next"
            | "previous"
            | "back"
            | "personal"
            | "defaults"
            | "custom"
            | "agent:quick"
            | "agent:thorough"
            | "workspace:prepared"
            | "workspace:clean"
            | "sandbox:on_demand"
            | "sandbox:always_live"
            | "private"
            | "public"
            | "groups"
            | "/create"
    ) || value.starts_with("org:")
        || value.starts_with("service:")
        || value.starts_with("bot:")
        || (lower == "create" && value.len() <= 64)
}
