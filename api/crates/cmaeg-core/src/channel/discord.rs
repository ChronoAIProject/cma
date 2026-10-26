//! Native Discord source validation; credentials never enter public source metadata.
use super::{validation::validate_admission, AdmissionPolicy, ChannelSecrets};
use crate::ValidationError;

pub(crate) fn valid_id(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|id| id > 0 && id.to_string() == value)
}
pub(super) fn validate(
    bot_id: &str,
    intents: u64,
    admission: &AdmissionPolicy,
    secrets: &ChannelSecrets,
    pointer: &str,
) -> Result<(), ValidationError> {
    if !valid_id(bot_id) {
        return Err(ValidationError::new(
            format!("{pointer}/bot_id"),
            "must be a canonical Discord ID",
        ));
    }
    const ALLOWED: u64 = 1 | (1 << 9) | (1 << 12) | (1 << 15);
    if intents & (1 << 15) == 0 || intents & ((1 << 9) | (1 << 12)) == 0 || intents & !ALLOWED != 0
    {
        return Err(ValidationError::new(format!("{pointer}/intents"), "requires MESSAGE_CONTENT and guild or direct messages; only message intents are supported"));
    }
    if secrets.discord_bot_token.as_ref().is_none_or(|token| {
        token.as_str().is_empty()
            || token.as_str().len() > 2048
            || !token.as_str().bytes().all(|b| (33..=126).contains(&b))
    }) {
        return Err(ValidationError::new(
            "/secrets/discord_bot_token",
            "requires 1..=2048 printable bearer bytes",
        ));
    }
    validate_admission(admission, pointer)
}
