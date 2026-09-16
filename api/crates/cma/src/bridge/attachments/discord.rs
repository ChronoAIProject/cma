//! Exact original Discord fields admitted under NyxID's signed body contract.
use layer2_core::bridge::ingress::{
    discord_snowflake, AuthenticatedCallbackPayload, CallbackError, DiscordTextAttachment,
    StoredInbox, VerifiedTextAttachment, MAX_CHANNEL_TEXT_ATTACHMENT_BYTES,
};
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// This is called only after Discord's original-message/human/loop classifier.
/// Unsupported files keep whole-message attachment_download_required behavior.
pub(crate) fn original(
    raw: &Value,
    inbox: &StoredInbox,
) -> Option<(DiscordTextAttachment, String)> {
    if inbox.envelope.platform != "discord"
        || !inbox.envelope.attachments.is_empty()
        || inbox.envelope.content_type != "text"
        || raw.get("op")?.as_u64()? != 0
        || raw.get("t")?.as_str()? != "MESSAGE_CREATE"
    {
        return None;
    }
    let message = raw.get("d")?;
    let channel = message.get("channel_id")?.as_str()?;
    let sender = message.pointer("/author/id")?.as_str()?;
    if channel != inbox.envelope.chat_id
        || sender != inbox.envelope.sender_id
        || !discord_snowflake(channel)
        || !discord_snowflake(sender)
        || !discord_snowflake(message.get("id")?.as_str()?)
        || message.pointer("/author/bot")?.as_bool()?
        || message.get("webhook_id").is_some()
        || message
            .pointer("/author/system")
            .is_some_and(|v| v.as_bool() != Some(false))
        || message
            .get("edited_timestamp")
            .is_some_and(|v| !v.is_null())
        || !matches!(message.get("type")?.as_u64()?, 0 | 19)
        || inbox.envelope.text.as_deref() != Some(message.get("content")?.as_str()?)
        || message.get("poll").is_some()
        || ["embeds", "components", "sticker_items"]
            .iter()
            .any(|name| {
                message
                    .get(name)
                    .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
            })
    {
        return None;
    }
    let files = message.get("attachments")?.as_array()?;
    if files.len() != 1 {
        return None;
    }
    let file = &files[0];
    if file.get("content_type")?.as_str()? != "text/plain"
        || file
            .get("ephemeral")
            .is_some_and(|v| v.as_bool() != Some(false))
        || file.get("flags").is_some_and(|v| v.as_u64() != Some(0))
    {
        return None;
    }
    let attachment_id = file.get("id")?.as_str()?.to_owned();
    let filename = file.get("filename")?.as_str()?.to_owned();
    let size_bytes = file.get("size")?.as_u64()?;
    if size_bytes == 0 || size_bytes > MAX_CHANNEL_TEXT_ATTACHMENT_BYTES as u64 {
        return None;
    }
    let url = file.get("url")?.as_str()?.to_owned();
    let expires = signed_url(&url, channel, &attachment_id, &filename, inbox.accepted_at)?;
    let metadata = DiscordTextAttachment {
        attachment_id,
        filename,
        size_bytes,
        source_url_sha256: format!("{:x}", Sha256::digest(url.as_bytes())),
        source_expires_at: expires,
        custody_expires_at: expires
            .min(inbox.expires_at)
            .min(inbox.accepted_at.saturating_add(900)),
        captured: None,
    };
    metadata.validate(inbox).ok()?;
    Some((metadata, url))
}

pub(crate) fn authenticated(
    payload: &AuthenticatedCallbackPayload<'_>,
) -> Result<Option<VerifiedTextAttachment>, CallbackError> {
    let callback: Value =
        serde_json::from_slice(payload.body()).map_err(|_| CallbackError::Payload)?;
    let candidate = callback
        .get("raw_platform_data")
        .and_then(|raw| original(raw, payload.record()));
    candidate
        .map(|(metadata, url)| {
            VerifiedTextAttachment::from_connector(payload.record(), metadata, url)
        })
        .transpose()
}

/// Fixed host and exact unencoded path; exactly three signed query fields.
/// We do not verify Discord's MAC ourselves: authority comes from the original
/// platform object authenticated by the configured NyxID connector. CDN checks
/// the MAC; forged/expired URLs can only fail the bounded download.
pub(super) fn signed_url(
    value: &str,
    channel: &str,
    attachment: &str,
    filename: &str,
    now: i64,
) -> Option<i64> {
    if value.len() > 4096
        || !value.starts_with("https://cdn.discordapp.com/")
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if url.as_str() != value
        || url.scheme() != "https"
        || url.host_str() != Some("cdn.discordapp.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || url.path() != format!("/attachments/{channel}/{attachment}/{filename}")
    {
        return None;
    }
    let query = url.query()?;
    let mut expires = None;
    let mut issued = None;
    let mut mac = None;
    for field in query.split('&') {
        let (key, value) = field.split_once('=')?;
        if !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return None;
        }
        match key {
            "ex" if expires.is_none() && value.len() == 8 => {
                expires = Some(i64::from_str_radix(value, 16).ok()?)
            }
            "is" if issued.is_none() && value.len() == 8 => {
                issued = Some(i64::from_str_radix(value, 16).ok()?)
            }
            "hm" if mac.is_none() && value.len() == 64 => mac = Some(()),
            _ => return None,
        }
    }
    let expires = expires?;
    let issued = issued?;
    mac?;
    (issued <= now.saturating_add(60)
        && expires > now
        && expires > issued
        && expires.saturating_sub(issued) <= 7 * 24 * 3600)
        .then_some(expires)
}
