//! Shared strict Discord facts for native and authenticated legacy relay ingress.
use crate::ingress::relay::VerificationError;
use serde_json::Value;
const INVALID: VerificationError = VerificationError("discord_event_invalid");
const IDENTITY: VerificationError = VerificationError("discord_identity_mismatch");
fn numeric_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()))
}
pub(crate) fn bool_fact(object: &Value, field: &str) -> Result<bool, VerificationError> {
    match object.get(field) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(INVALID),
    }
}

pub(crate) fn mentions(
    message: &Value,
    bot: Option<&str>,
) -> Result<Option<bool>, VerificationError> {
    let mentions = match message.get("mentions") {
        None => &[][..],
        Some(Value::Array(values)) => values.as_slice(),
        _ => return Err(INVALID),
    };
    Ok(bot.map(|bot| {
        mentions
            .iter()
            .any(|user| numeric_id(&user["id"]) == Some(bot))
    }))
}

pub(crate) fn reply_author(
    message: &Value,
    parent: &str,
    chat: &str,
    bot: Option<&str>,
) -> Result<Option<bool>, VerificationError> {
    let Some(reply) = message.get("referenced_message").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    if numeric_id(&reply["id"]) != Some(parent)
        || reply["channel_id"].as_str().is_some_and(|id| id != chat)
        || message["message_reference"]["channel_id"]
            .as_str()
            .is_some_and(|id| id != chat)
    {
        return Err(IDENTITY);
    }
    let author = numeric_id(&reply["author"]["id"]).ok_or(INVALID)?;
    Ok(bot.map(|id| id == author))
}
