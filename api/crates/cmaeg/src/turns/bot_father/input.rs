use cmaeg_core::activity::ActivityKind;
use serde_json::{json, Value};

/// Return whether this event is an explicit continuation of the durable Bot
/// setup workflow. Ordinary messages stay on the provider turn so Bot Father
/// remains a normal agent. The model's Bot Father skill handles natural
/// language intent; this gate only preserves callbacks and documented
/// text/JSON controls emitted by setup cards.
pub(super) fn is_setup_activity(activity: &cmaeg_core::activity::Activity) -> bool {
    match &activity.kind {
        ActivityKind::Interaction(_) => true,
        ActivityKind::Command { name, .. } => matches!(name.as_str(), "start" | "create"),
        ActivityKind::Message { text, .. } => is_setup_text(text),
        ActivityKind::Event { .. } => false,
    }
}

fn is_setup_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('{')
        || trimmed.starts_with("```")
        || trimmed == "/start"
        || trimmed.starts_with("/start link_")
        || trimmed.starts_with("link_")
        || trimmed.starts_with("name:")
        || trimmed.starts_with("instructions:")
        || trimmed.starts_with("model:")
        || trimmed.starts_with("repository:")
        || trimmed.starts_with("directory:")
        || trimmed.starts_with("resources:")
    {
        return true;
    }
    setup_command(trimmed)
}

/// The gateway translates the documented text envelope, while CMA validates
/// profile fields, service authority and the current workflow step.
pub(super) fn request(activity: &cmaeg_core::activity::Activity) -> Result<(Value, Value), String> {
    let ActivityKind::Message { text, .. } = &activity.kind else {
        return Ok((json!({}), json!({})));
    };
    let text = text.trim();
    if !text.starts_with('{') && !text.starts_with("```") {
        return Ok((draft(activity), json!({})));
    }
    let invalid = || {
        "Send the complete JSON template from the current Customize step: {\"agent\": {...}}, {\"workspace\": {...}}, or {\"sandbox\": {...}, \"availability\": {...}}. Keep the outer object and use valid JSON; you can also reply `back` or `cancel`.".to_owned()
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
    let ActivityKind::Message { text, .. } = &activity.kind else {
        return json!({});
    };
    let trimmed = text.trim();
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
