use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::hmac;

/// Standard Webhooks headers for the exact serialized event body.
pub struct WebhookHeaders {
    /// Stable webhook event identity.
    pub id: String,
    /// Unix timestamp in decimal seconds, unchanged from the caller.
    pub timestamp: String,
    /// CMAEG's `v1,` signature. Do not log signed request headers.
    pub signature: String,
}

/// Sign `id.timestamp.body` with the UTF-8 secret string, without base64 decoding it.
pub fn sign_webhook(
    secret: &str,
    id: &str,
    timestamp: &str,
    body: &[u8],
) -> Result<WebhookHeaders> {
    if id.is_empty()
        || id.len() > 512
        || !id.bytes().all(|b| (33..=126).contains(&b))
        || timestamp.is_empty()
        || timestamp.len() > 20
        || !timestamp.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::validation("webhook id or timestamp is invalid"));
    }
    if secret.is_empty() || secret.len() > 4096 {
        return Err(Error::validation(
            "webhook secret must contain 1–4096 UTF-8 bytes",
        ));
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut context = hmac::Context::with_key(&key);
    for bytes in [id.as_bytes(), b".", timestamp.as_bytes(), b".", body] {
        context.update(bytes);
    }
    Ok(WebhookHeaders {
        id: id.into(),
        timestamp: timestamp.into(),
        signature: format!("v1,{}", STANDARD.encode(context.sign().as_ref())),
    })
}
