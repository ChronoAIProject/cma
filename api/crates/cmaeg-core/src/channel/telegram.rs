use super::*;

pub(super) fn validate(
    bot_id: &str,
    username: Option<&str>,
    admission: &AdmissionPolicy,
    secrets: &ChannelSecrets,
    pointer: &str,
) -> Result<(), ValidationError> {
    if !bot_id
        .parse::<i64>()
        .is_ok_and(|id| id > 0 && id.to_string() == bot_id)
    {
        return Err(ValidationError::new(
            format!("{pointer}/bot_id"),
            "must be a positive Telegram bot ID",
        ));
    }
    validate_relay_identity(
        Platform::Telegram,
        username,
        Some(bot_id),
        false,
        false,
        pointer,
    )?;
    validate_admission(admission, pointer)?;
    let token = secrets
        .telegram_bot_token
        .as_ref()
        .map(Secret::as_str)
        .unwrap_or_default();
    let Some((id, key)) = token.split_once(':') else {
        return Err(ValidationError::new(
            "/secrets/telegram_bot_token",
            "a Telegram token is required",
        ));
    };
    if id != bot_id
        || key.is_empty()
        || key.len() > 256
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(ValidationError::new(
            "/secrets/telegram_bot_token",
            "must match the Telegram bot identity",
        ));
    }
    let secret = secrets
        .telegram_webhook_secret
        .as_ref()
        .map(Secret::as_str)
        .unwrap_or_default();
    if secret.is_empty()
        || secret.len() > 256
        || !secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(ValidationError::new(
            "/secrets/telegram_webhook_secret",
            "must contain 1..=256 Telegram secret characters",
        ));
    }
    Ok(())
}
