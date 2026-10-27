//! Bounded serialization and receipt checks for the NyxID relay exchange.
use super::{Credential, DeliveryOutcome, Secret};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
pub(super) fn credential_secret(credential: Credential<'_>) -> Result<&Secret, DeliveryOutcome> {
    let (Credential::ReplyToken(secret) | Credential::ChannelKey(secret)) = credential;
    if secret.as_str().is_empty() || !secret.as_str().bytes().all(|b| (33..=126).contains(&b)) {
        return Err(DeliveryOutcome::Rejected {
            reason: "no_credential".into(),
        });
    }
    Ok(secret)
}
pub(super) fn serialize_reply(
    message_id: &str,
    text: &str,
    markup: Option<&serde_json::Value>,
    thread_reply: bool,
) -> Result<Vec<u8>, serde_json::Error> {
    let mut body = serde_json::json!({"message_id":message_id,"reply":{"text":text,"metadata":markup.map(|value|serde_json::json!({"reply_markup":value}))}});
    if thread_reply {
        body["thread_reply"] = true.into();
    }
    serde_json::to_vec(&body)
}
/// The bearer and JSON headers reqwest's `bearer_auth` and `json` set, with the
/// credential marked sensitive. `None` when the credential is not a header value.
pub(super) fn headers(secret: &Secret) -> Option<HeaderMap> {
    let mut authorization = HeaderValue::try_from(format!("Bearer {}", secret.as_str())).ok()?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Some(headers)
}
pub(super) fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
pub(super) fn retry_after(headers: &HeaderMap) -> u64 {
    let Some(raw) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
    else {
        return 1;
    };
    if let Ok(seconds) = raw.parse::<u64>() {
        return seconds.clamp(1, 10);
    }
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc2822)
        .ok()
        .map(|at| {
            (at.unix_timestamp() - time::OffsetDateTime::now_utc().unix_timestamp()).clamp(1, 10)
                as u64
        })
        .unwrap_or(1)
}
