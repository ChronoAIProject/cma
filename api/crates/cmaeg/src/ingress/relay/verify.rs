//! Complete callback, optional relay-user, and reply-token binding checks.
use super::{
    attachments::RepairField,
    envelope_errors::rejected_envelope,
    jwks::JwksCache,
    jwt::{StandardClaims, Token},
    payload::{identifier, timestamp, CallbackPayload},
    VerificationError,
};
use crate::channels::resolver::ResolvedChannel;
use axum::http::HeaderMap;
use cmaeg_core::channel::{Secret, Source};
use serde::{de::DeserializeOwned, Deserialize};
use sha2::{Digest, Sha256};

pub struct CallbackHeaders<'a> {
    pub callback_token: &'a str,
    pub user_token: Option<&'a str>,
    pub message_id: &'a str,
    pub platform: &'a str,
    pub timestamp: &'a str,
}
impl<'a> CallbackHeaders<'a> {
    pub fn read(headers: &'a HeaderMap) -> Result<Self, VerificationError> {
        Ok(Self {
            callback_token: single(headers, "x-nyxid-callback-token", true)?.unwrap(),
            message_id: single(headers, "x-nyxid-message-id", true)?.unwrap(),
            timestamp: single(headers, "x-nyxid-timestamp", true)?.unwrap(),
            platform: single(headers, "x-nyxid-platform", true)?.unwrap(),
            user_token: single(headers, "x-nyxid-user-token", false)?,
        })
    }
}
pub fn single<'a>(
    headers: &'a HeaderMap,
    name: &str,
    required: bool,
) -> Result<Option<&'a str>, VerificationError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(VerificationError("header_duplicate"));
    }
    match first {
        None if required => Err(VerificationError("header_missing")),
        None => Ok(None),
        Some(value) => value
            .to_str()
            .ok()
            .filter(|value| !value.is_empty() && !value.contains(','))
            .map(Some)
            .ok_or(VerificationError("header_invalid")),
    }
}
#[derive(Deserialize)]
struct CallbackClaims {
    #[serde(flatten)]
    standard: StandardClaims,
    api_key_id: String,
    message_id: String,
    platform: String,
    body_sha256: String,
}
#[derive(Deserialize)]
struct ReplyClaims {
    #[serde(flatten)]
    standard: StandardClaims,
    api_key_id: String,
    conversation_id: String,
    inbound_message_id: String,
    platform: String,
}
#[derive(Deserialize)]
struct RelayClaims {
    #[serde(flatten)]
    standard: StandardClaims,
    sub: String,
    relay: bool,
    relay_api_key_id: String,
    relay_allowed_service_ids: Vec<String>,
    relay_allowed_node_ids: Vec<String>,
    relay_allow_all_services: bool,
    relay_allow_all_nodes: bool,
    act: Option<serde_json::Value>,
    delegated: Option<bool>,
    sa: Option<bool>,
    cnf: Option<serde_json::Value>,
    assistant_forward: Option<bool>,
}
pub struct VerifiedCallback {
    pub payload: CallbackPayload,
    pub reply_token: Secret,
    pub reply_expires_at: i64,
    pub callback_deadline: i64,
}
pub async fn verify(
    channel: &ResolvedChannel,
    source_index: usize,
    keys: &JwksCache,
    headers: CallbackHeaders<'_>,
    body: &[u8],
    now: i64,
) -> Result<VerifiedCallback, VerificationError> {
    let Some(Source::NyxidRelay {
        issuer,
        key_id,
        route_ids,
        platform,
        ..
    }) = channel.definition.sources.get(source_index)
    else {
        return Err(VerificationError("relay_source_missing"));
    };
    if body.is_empty()
        || headers.callback_token.len() > 16_384
        || headers.user_token.is_some_and(|token| token.len() > 16_384)
        || !identifier(headers.message_id)
        || !identifier(headers.platform)
        || headers.timestamp.len() > 64
    {
        return Err(VerificationError("callback_bounds"));
    }
    let claims: CallbackClaims =
        authenticated(channel, source_index, keys, headers.callback_token).await?;
    claims
        .standard
        .require(issuer, "channel-relay/callback", "relay_callback", 300, now)?;
    if claims.body_sha256 != format!("{:x}", Sha256::digest(body)) {
        return Err(VerificationError("body_digest_mismatch"));
    }
    if claims.api_key_id != *key_id {
        return Err(VerificationError("callback_key_mismatch"));
    }
    if !platform.matches_wire(&claims.platform) || !platform.matches_wire(headers.platform) {
        return Err(VerificationError("platform_mismatch"));
    }
    if claims.message_id != headers.message_id {
        return Err(VerificationError("message_mismatch"));
    }
    if timestamp(headers.timestamp).is_none_or(|time| time.abs_diff(claims.standard.iat) > 60) {
        return Err(VerificationError("timestamp_mismatch"));
    }
    let mut payload: CallbackPayload =
        serde_json::from_slice(body).map_err(|error| rejected_envelope(&error))?;
    // The body digest is verified, so these repairs describe authenticated NyxID output.
    if payload.sender.repair_display_name() {
        let repairs = &mut payload.content.attachments.repairs;
        repairs.record(RepairField::SenderDisplayName);
    }
    payload.content.attachments.repairs.record_metrics();
    payload.validate()?;
    if payload.message_id != claims.message_id
        || !platform.matches_wire(&payload.platform)
        || payload.correlation_id != claims.standard.jti
        || payload.agent.api_key_id != *key_id
    {
        return Err(VerificationError("payload_binding_mismatch"));
    }
    if !route_ids.contains(&payload.conversation.id) {
        return Err(VerificationError("route_denied"));
    }
    if let Some(token) = headers.user_token {
        let relay: RelayClaims = authenticated(channel, source_index, keys, token).await?;
        relay.standard.require(issuer, issuer, "access", 300, now)?;
        if !relay.relay
            || relay.sub != channel.definition.owner.subject
            || relay.relay_api_key_id != *key_id
            || relay.act.is_some()
            || relay.delegated.unwrap_or(false)
            || relay.sa.unwrap_or(false)
            || relay.cnf.is_some()
            || relay.assistant_forward.unwrap_or(false)
            || [
                &relay.relay_allowed_service_ids,
                &relay.relay_allowed_node_ids,
            ]
            .iter()
            .any(|ids| ids.len() > 256 || ids.iter().any(|id| !identifier(id)))
        {
            return Err(VerificationError("relay_user_mismatch"));
        }
        // Inherited scopes are corroboration only, never an invocation credential.
        let _ = (relay.relay_allow_all_services, relay.relay_allow_all_nodes);
    }
    let reply_token = payload
        .reply_token
        .take()
        .ok_or(VerificationError("reply_token_missing"))?;
    let reply: ReplyClaims =
        authenticated(channel, source_index, keys, reply_token.as_str()).await?;
    reply
        .standard
        .require(issuer, "channel-relay/reply", "relay_reply", 1800, now)?;
    if reply.api_key_id != *key_id
        || reply.conversation_id != payload.conversation.id
        || reply.inbound_message_id != payload.message_id
        || !platform.matches_wire(&reply.platform)
    {
        return Err(VerificationError("reply_binding_mismatch"));
    }
    if reply.standard.exp <= now {
        return Err(VerificationError("reply_expired"));
    }
    Ok(VerifiedCallback {
        payload,
        reply_token,
        reply_expires_at: reply.standard.exp,
        callback_deadline: claims.standard.exp.saturating_add(60),
    })
}
async fn authenticated<T: DeserializeOwned>(
    channel: &ResolvedChannel,
    source_index: usize,
    keys: &JwksCache,
    token: &str,
) -> Result<T, VerificationError> {
    let token = Token::parse(token)?;
    keys.keys_for(channel, source_index, &token.kid)
        .await?
        .verify(&token)
}
