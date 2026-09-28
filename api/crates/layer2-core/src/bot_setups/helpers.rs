use super::*;
use crate::{bots::BotError, resource_access::ResourceOwner};

pub(super) fn grant(setup: &BotSetup) -> Result<&str, BotError> {
    setup.grant_id.as_deref().ok_or(BotError::Forbidden)
}
pub(super) fn owner(setup: &BotSetup) -> Result<&ResourceOwner, BotError> {
    setup.owner.as_ref().ok_or(BotError::Forbidden)
}
pub(super) fn invalid_action() -> BotError {
    BotError::Invalid("Choose an option from the current configuration.".into())
}

/// Resolve the text command used by relays that cannot carry interactive
/// callbacks.  Telegram's existing relay forwards the user's reply as normal
/// text, so Bot Father renders the exact action IDs and accepts those IDs on
/// the next turn.  The short `create` alias is expanded against the durable
/// review digest instead of trusting a digest supplied by the client.
pub(super) fn text_action(setup: &BotSetup, text: &str) -> Option<String> {
    let value = text
        .trim()
        .strip_prefix("/choice ")
        .or_else(|| text.trim().strip_prefix("choice "))
        .unwrap_or(text.trim());
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_whitespace) {
        return None;
    }
    // Cancel is available even before authentication or naming is complete.
    if value.eq_ignore_ascii_case("cancel") {
        return Some("cancel".into());
    }
    if matches!(setup.step, SetupStep::Login | SetupStep::Name) {
        return None;
    }
    if value.eq_ignore_ascii_case("create") && setup.step == SetupStep::Review {
        return setup
            .confirmation_digest
            .as_ref()
            .map(|digest| format!("create:{digest}"));
    }
    let lower = value.to_ascii_lowercase();
    let simple = matches!(
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
    );
    if simple {
        return Some(lower);
    }
    if value.starts_with("org:") || value.starts_with("service:") || value.starts_with("bot:") {
        return Some(value.to_owned());
    }
    None
}
pub(super) fn choose(
    action: &str,
    defaults: SetupStep,
    custom: SetupStep,
) -> Result<SetupStep, BotError> {
    match action {
        "defaults" => Ok(defaults),
        "custom" => Ok(custom),
        _ => Err(invalid_action()),
    }
}
pub(super) fn login_proof(text: &str) -> Option<&str> {
    let text = text.trim();
    text.strip_prefix("/start link_")
        .or_else(|| text.strip_prefix("link_"))
        .filter(|proof| {
            !proof.is_empty()
                && proof.len() <= 128
                && proof
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}
pub(super) fn apply_draft(setup: &mut BotSetup, draft: &SetupDraft) -> Result<(), BotError> {
    if let Some(name) = &draft.name {
        if name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
            return Err(BotError::Invalid(
                "Bot name must contain 1–80 characters.".into(),
            ));
        }
        setup.draft.name = Some(name.trim().into());
    }
    if let Some(instructions) = &draft.instructions {
        if instructions.len() > 32 * 1024 {
            return Err(BotError::Invalid(
                "Agent instructions exceed 32 KiB.".into(),
            ));
        }
        setup.draft.instructions = Some(instructions.clone());
        setup.configuration.agent.system_prompt = instructions.clone();
    }
    Ok(())
}
