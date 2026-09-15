use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::bridge::{BridgeAdmissionPolicy, BridgePlatform, BridgeRecord, BridgeReplyPolicy};
use crate::resource_access::ResourceOwner;

use super::jwt::StandardClaims;
use super::payload::{identifier, Payload};
use super::*;

/// The HTTP adapter must reject duplicate sensitive headers before constructing
/// this value. These headers are never logged or serialized by the core.
pub struct CallbackHeaders<'a> {
    pub callback_token: &'a str,
    pub user_token: Option<&'a str>,
    pub message_id: &'a str,
    pub platform: &'a str,
    pub timestamp: &'a str,
}

pub struct CallbackVerifier {
    policy: CallbackTokenPolicy,
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

impl CallbackVerifier {
    pub fn new(policy: CallbackTokenPolicy) -> Result<Self, CallbackError> {
        policy.validate()?;
        Ok(Self { policy })
    }

    pub fn verify(
        &self,
        binding: CallbackBinding,
        keys: &CallbackKeySet,
        headers: CallbackHeaders<'_>,
        body: &[u8],
    ) -> Result<InboxWrite, CallbackError> {
        binding.require_current(&binding.bridge)?;
        self.preauthenticate(binding.bridge.clone(), keys, headers, body)?
            .bind(binding)
    }

    pub fn preauthenticate(
        &self,
        bridge: BridgeRecord,
        keys: &CallbackKeySet,
        headers: CallbackHeaders<'_>,
        body: &[u8],
    ) -> Result<SignedCallback, CallbackError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CallbackError::Authentication)?
            .as_secs();
        self.preauthenticate_at(
            bridge,
            keys,
            headers,
            body,
            i64::try_from(now).map_err(|_| CallbackError::Authentication)?,
        )
    }

    fn preauthenticate_at(
        &self,
        bridge: BridgeRecord,
        keys: &CallbackKeySet,
        headers: CallbackHeaders<'_>,
        body: &[u8],
        now: i64,
    ) -> Result<SignedCallback, CallbackError> {
        let started = tokio::time::Instant::now();
        bridge
            .admission_pin()
            .map_err(|_| CallbackError::BindingUnavailable)?;
        let config = &bridge
            .current()
            .map_err(|_| CallbackError::BindingUnavailable)?
            .config;
        if body.is_empty()
            || body.len() > config.limits.max_payload_bytes as usize
            || body.len() > 1048576
            || headers.callback_token.len() > 16384
            || headers.user_token.is_some_and(|s| s.len() > 16384)
            || !identifier(headers.message_id)
            || !identifier(headers.platform)
            || headers.timestamp.len() > 64
        {
            return Err(CallbackError::Bounds);
        }
        if self.policy.issuer != config.connector.issuer
            || self.policy.relay_audience != config.connector.base_url
        {
            return Err(CallbackError::BindingUnavailable);
        }
        let platform = match config.connector.platform {
            BridgePlatform::Telegram => "telegram",
            BridgePlatform::Discord => "discord",
            BridgePlatform::Lark => "lark",
            BridgePlatform::Feishu => "feishu",
        };
        let (claims, kid): (CallbackClaims, _) =
            keys.verify(headers.callback_token, &self.policy.issuer)?;
        claims.standard.require(
            &self.policy,
            "channel-relay/callback",
            "relay_callback",
            self.policy.callback_ttl_seconds,
            now,
        )?;
        let digest = format!("{:x}", Sha256::digest(body));
        let header_time = super::timestamp::provider_timestamp(headers.timestamp)
            .ok_or(CallbackError::Authentication)?;
        if claims.body_sha256 != digest
            || claims.api_key_id != config.connector.key_id
            || claims.platform != platform
            || claims.message_id != headers.message_id
            || headers.platform != platform
            || header_time.abs_diff(claims.standard.iat) > u64::from(self.policy.clock_skew_seconds)
        {
            return Err(CallbackError::Authentication);
        }
        let mut payload: Payload =
            serde_json::from_slice(body).map_err(|_| CallbackError::Payload)?;
        payload.validate()?;
        if payload.message_id != claims.message_id
            || payload.correlation_id != claims.standard.jti
            || payload.platform != platform
            || payload.agent.api_key_id != config.connector.key_id
            || !config
                .connector
                .route_ids
                .contains(&payload.conversation.id)
        {
            return Err(CallbackError::Authentication);
        }
        let relay = self.relay(
            keys,
            headers.user_token,
            &bridge.owner,
            &config.connector.key_id,
            now,
        )?;
        let reply_token = payload.reply_token.take().map(Zeroizing::new);
        // NyxID's generic thread context may contain Discord interaction
        // credentials. It is never a stable partition ID or plaintext field.
        let thread_context = payload.thread_id.take().map(Zeroizing::new);
        let reply_expires = reply_token
            .as_ref()
            .map(|token| {
                let (reply, _): (ReplyClaims, _) = keys.verify(token, &self.policy.issuer)?;
                reply.standard.require(
                    &self.policy,
                    "channel-relay/reply",
                    "relay_reply",
                    self.policy.reply_ttl_seconds,
                    now,
                )?;
                if reply.api_key_id != config.connector.key_id
                    || reply.conversation_id != payload.conversation.id
                    || reply.inbound_message_id != payload.message_id
                    || reply.platform != platform
                    || reply.standard.exp <= now
                {
                    return Err(CallbackError::Authentication);
                }
                Ok(reply.standard.exp.min(
                    now.saturating_add(i64::from(config.limits.reply_secret_retention_seconds)),
                ))
            })
            .transpose()?;
        let admission = match &config.admission {
            BridgeAdmissionPolicy::All => InboxAdmission::Accepted,
            BridgeAdmissionPolicy::Allowlist { sender_ids }
                if sender_ids.contains(&payload.sender.platform_id) =>
            {
                InboxAdmission::Accepted
            }
            _ => InboxAdmission::SenderDenied,
        };
        let retain_reply =
            admission == InboxAdmission::Accepted && config.reply != BridgeReplyPolicy::Disabled;
        // Binding + business message identity, deliberately excluding JWT jti
        // and config revision so a newly signed duplicate keeps one receipt.
        let identity = serde_json::to_vec(&(
            &self.policy.issuer,
            &bridge.id,
            &config.connector.bot_id,
            &config.connector.key_id,
            &payload.message_id,
        ))
        .map_err(|_| CallbackError::Payload)?;
        let mut record = StoredInbox {
            id: format!("{:x}", Sha256::digest(identity)),
            owner: bridge.owner.clone(),
            pin: bridge
                .admission_pin()
                .map_err(|_| CallbackError::BindingUnavailable)?,
            bot_id: config.connector.bot_id.clone(),
            key_id: config.connector.key_id.clone(),
            message_id: payload.message_id.clone(),
            correlation_id: payload.correlation_id.clone(),
            envelope: payload.normalize(),
            attestation: CallbackAttestation {
                issuer: claims.standard.iss,
                signing_key_id: kid,
                token_id: claims.standard.jti,
                issued_at: claims.standard.iat,
                expires_at: claims.standard.exp,
                body_sha256: digest,
            },
            relay,
            admission,
            accepted_at: now,
            expires_at: now.saturating_add(i64::from(config.limits.inbox_retention_seconds)),
            reply_expires_at: if retain_reply {
                reply_expires.or_else(|| {
                    thread_context.as_ref().map(|_| {
                        now.saturating_add(i64::from(config.limits.reply_secret_retention_seconds))
                    })
                })
            } else {
                None
            },
        };
        record.envelope.protected_thread_context_present = retain_reply && thread_context.is_some();
        let remaining = claims
            .standard
            .exp
            .saturating_add(i64::from(self.policy.clock_skew_seconds))
            .saturating_sub(now);
        let deadline = started + Duration::from_secs(remaining.clamp(0, 5) as u64);
        let signed = SignedCallback {
            bridge,
            record,
            reply_token: if retain_reply { reply_token } else { None },
            thread_context: if retain_reply { thread_context } else { None },
            deadline: deadline.min(keys.deadline()),
        };
        if tokio::time::Instant::now() >= signed.deadline {
            return Err(CallbackError::Authentication);
        }
        Ok(signed)
    }

    fn relay(
        &self,
        keys: &CallbackKeySet,
        token: Option<&str>,
        owner: &ResourceOwner,
        key: &str,
        now: i64,
    ) -> Result<RelayCorroboration, CallbackError> {
        let Some(token) = token else {
            return Ok(RelayCorroboration::AbsentInboxOnly);
        };
        let (claims, _): (RelayClaims, _) = keys.verify(token, &self.policy.issuer)?;
        claims.standard.require(
            &self.policy,
            &self.policy.relay_audience,
            "access",
            self.policy.relay_ttl_seconds,
            now,
        )?;
        let subject = match owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if !claims.relay
            || claims.sub != subject.as_str()
            || claims.relay_api_key_id != key
            || claims.act.is_some()
            || claims.delegated.unwrap_or(false)
            || claims.sa.unwrap_or(false)
            || claims.cnf.is_some()
            || claims.assistant_forward.unwrap_or(false)
            || [
                &claims.relay_allowed_service_ids,
                &claims.relay_allowed_node_ids,
            ]
            .iter()
            .any(|ids| ids.len() > 256 || ids.iter().any(|id| !identifier(id)))
        {
            return Err(CallbackError::Authentication);
        }
        // Scope flags are parsed strictly and discarded: this foundation never
        // turns inherited relay scope into a usable execution credential.
        let _ = (
            claims.relay_allow_all_services,
            claims.relay_allow_all_nodes,
        );
        Ok(RelayCorroboration::VerifiedInboxOnly {
            subject: claims.sub,
            key_id: claims.relay_api_key_id,
            expires_at: claims.standard.exp,
        })
    }
}
