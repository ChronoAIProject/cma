//! Standard Webhooks wire signatures over the exact body bytes.
use axum::http::HeaderMap;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::channel::Secret;
use ring::hmac;

pub fn signature(secret: &Secret, id: &str, timestamp: &str, body: &[u8]) -> String {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, secret.as_str().as_bytes()),
        &signed_bytes(id, timestamp, body),
    );
    format!("v1,{}", STANDARD.encode(tag.as_ref()))
}
pub fn verify(headers: &HeaderMap, secret: &Secret, body: &[u8], now: i64) -> bool {
    let (Some(id), Some(timestamp), Some(signatures)) = (
        single(headers, "webhook-id"),
        single(headers, "webhook-timestamp"),
        single(headers, "webhook-signature"),
    ) else {
        return false;
    };
    if id.is_empty()
        || id.len() > 512
        || timestamp.len() > 20
        || !timestamp.bytes().all(|byte| byte.is_ascii_digit())
        || signatures.len() > 2048
    {
        return false;
    }
    let Ok(seconds) = timestamp.parse::<i64>() else {
        return false;
    };
    if seconds.abs_diff(now) > 300 || secret.as_str().is_empty() {
        return false;
    }
    let bytes = signed_bytes(id, timestamp, body);
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_str().as_bytes());
    let mut valid = false;
    for entry in signatures.split_ascii_whitespace().take(16) {
        if let Some(encoded) = entry.strip_prefix("v1,") {
            if let Ok(tag) = STANDARD.decode(encoded) {
                valid |= hmac::verify(&key, &bytes, &tag).is_ok();
            }
        }
    }
    valid
}
pub(crate) fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(first)
}
fn signed_bytes(id: &str, timestamp: &str, body: &[u8]) -> Vec<u8> {
    let mut signed = format!("{id}.{timestamp}.").into_bytes();
    signed.extend_from_slice(body);
    signed
}
