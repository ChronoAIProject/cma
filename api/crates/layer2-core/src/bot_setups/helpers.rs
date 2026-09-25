use super::*;
use crate::bots::BotError;
use crate::resource_access::ResourceOwner;

pub(super) fn grant(setup: &BotSetup) -> Result<&str, BotError> {
    setup.grant_id.as_deref().ok_or(BotError::Forbidden)
}
pub(super) fn owner(setup: &BotSetup) -> Result<&ResourceOwner, BotError> {
    setup.owner.as_ref().ok_or(BotError::Forbidden)
}
pub(super) fn invalid_action() -> BotError {
    BotError::Invalid("Choose an option from the current configuration.".into())
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
