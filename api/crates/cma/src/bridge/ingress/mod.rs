//! Dedicated signed-callback adapter. Production routes stay closed while
//! selected-key custody is unavailable. No callback claims can mint a binding.
use std::{sync::Arc, time::Duration};

use axum::http::HeaderMap;
use layer2_core::bridge::ingress::{
    CallbackAcceptance, CallbackBinding, CallbackError, CallbackHeaders, CallbackInboxStore,
    CallbackTokenPolicy, CallbackVerifier,
};
use layer2_core::bridge::{BridgeConnectionValidator, BridgeId, BridgeStore};
use tokio::sync::Semaphore;

mod capabilities;
#[cfg(test)]
pub(crate) use capabilities::tests::{event_cases, fixtures::Fixture as CallbackFixture};
#[cfg(test)]
pub(crate) use capabilities::NyxidCapabilities;
mod configured;
pub(crate) mod controls;
pub(crate) mod http;
pub(crate) use configured::configured;
mod keys;
mod redelivery;
pub(crate) mod reply_addressing;
pub(crate) use keys::PinnedCallbackKeys;

pub(crate) struct CallbackIngress {
    bridges: Arc<dyn BridgeStore>,
    connections: Arc<dyn BridgeConnectionValidator>,
    inbox: Arc<dyn CallbackInboxStore>,
    keys: PinnedCallbackKeys,
    verifier: CallbackVerifier,
    callback_root: String,
    workers: Semaphore,
    reply_addressing: Option<Arc<dyn reply_addressing::ReplyAddressingStore>>,
    controls: Option<Arc<controls::ChannelControlIngress>>,
}

impl CallbackIngress {
    pub(crate) fn new(
        bridges: Arc<dyn BridgeStore>,
        connections: Arc<dyn BridgeConnectionValidator>,
        inbox: Arc<dyn CallbackInboxStore>,
        keys: PinnedCallbackKeys,
        policy: CallbackTokenPolicy,
        callback_root: String,
    ) -> Result<Self, CallbackError> {
        let url =
            reqwest::Url::parse(&callback_root).map_err(|_| CallbackError::BindingUnavailable)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || callback_root.ends_with('/')
        {
            return Err(CallbackError::BindingUnavailable);
        }
        Ok(Self {
            bridges,
            connections,
            inbox,
            keys,
            verifier: CallbackVerifier::new(policy)?,
            callback_root,
            workers: Semaphore::new(16),
            reply_addressing: None,
            controls: None,
        })
    }

    pub(crate) fn with_reply_addressing(
        mut self,
        store: Arc<dyn reply_addressing::ReplyAddressingStore>,
    ) -> Self {
        self.reply_addressing = Some(store);
        self
    }

    pub(crate) fn with_channel_controls(
        mut self,
        controls: Arc<controls::ChannelControlIngress>,
    ) -> Self {
        self.controls = Some(controls);
        self
    }

    /// The transport must also limit the streaming body to 1 MiB before buffering.
    /// Only durable success may map to HTTP 202. Timeout means acceptance is
    /// unknown; a valid retry may reconcile a committed receipt.
    pub(crate) async fn accept(
        &self,
        id: &BridgeId,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<CallbackAcceptance, CallbackError> {
        let _worker = self
            .workers
            .try_acquire()
            .map_err(|_| CallbackError::Capacity)?;
        if body.is_empty() || body.len() > 1048576 {
            return Err(CallbackError::Bounds);
        }
        let headers = parse_headers(headers)?;
        tokio::time::timeout(Duration::from_secs(5), self.accept_inner(id, headers, body))
            .await
            .map_err(|_| CallbackError::Unavailable)?
    }

    async fn accept_inner(
        &self,
        id: &BridgeId,
        headers: CallbackHeaders<'_>,
        body: &[u8],
    ) -> Result<CallbackAcceptance, CallbackError> {
        let bridge = self
            .bridges
            .bridge(id)
            .await
            .map_err(|_| CallbackError::BindingUnavailable)?
            .ok_or(CallbackError::BindingUnavailable)?;
        let config = &bridge
            .current()
            .map_err(|_| CallbackError::BindingUnavailable)?
            .config;
        if config.connector.callback_url != format!("{}/{}", self.callback_root, id.as_str()) {
            return Err(CallbackError::BindingUnavailable);
        }
        bridge
            .admission_pin()
            .map_err(|_| CallbackError::BindingUnavailable)?;
        if body.len() > config.limits.max_payload_bytes as usize {
            return Err(CallbackError::Bounds);
        }
        self.connections
            .check_feature(
                layer2_core::bridge::BridgeFeature::Ingress,
                &config.connector.platform,
            )
            .map_err(|_| CallbackError::BindingUnavailable)?;
        let keyset = self.keys.get(false).await?;
        let verify = |keys| {
            self.verifier.preauthenticate(
                bridge.clone(),
                keys,
                CallbackHeaders {
                    callback_token: headers.callback_token,
                    user_token: headers.user_token,
                    message_id: headers.message_id,
                    platform: headers.platform,
                    timestamp: headers.timestamp,
                },
                body,
            )
        };
        let signed = match verify(&keyset) {
            Err(CallbackError::KeysUnavailable) => verify(self.keys.get(true).await?.as_ref())?,
            result => result?,
        };
        let redelivery = redelivery::observe(&signed.authenticated_payload(body)?)?;
        let control_candidate = self.controls.is_some()
            && signed
                .authenticated_payload(body)?
                .record()
                .envelope
                .text
                .as_deref()
                .is_some_and(|text| {
                    layer2_core::bridge::channel_control::ChannelControlRequest::parse(text).is_ok()
                });
        let reply = if control_candidate
            || matches!(&config.admission,
            layer2_core::bridge::BridgeAdmissionPolicy::Scoped { policy }
                if matches!(policy.groups, layer2_core::bridge::partition::GroupAdmission::ReplyToBot
                    | layer2_core::bridge::partition::GroupAdmission::MentionOrReplyToBot))
        {
            if let Some(store) = &self.reply_addressing {
                let payload = signed.authenticated_payload(body)?;
                match reply_addressing::ReplyLookup::authenticated(&payload)? {
                    Some(lookup) => store.confirmed_reply(&lookup).await,
                    None => Ok(None),
                }
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        };
        let (reply, lookup_error) = match reply {
            Ok(reply) => (reply, None),
            Err(error) => (None, Some(error)),
        };
        let signed =
            signed.classify_capabilities(body, &reply_addressing::ReplyCapabilities(reply))?;
        // Only cryptographically authenticated and bounded callbacks may cause
        // selected-key custody or owner/provider inventory IO. The signed
        // snapshot alone still cannot authorize durable acceptance.
        let proof = self
            .connections
            .validate_connection(&bridge.owner, config)
            .await
            .map_err(|_| CallbackError::BindingUnavailable)?;
        let write = signed
            .bind(CallbackBinding::new(bridge, proof)?)?
            .with_platform_redelivery(redelivery)?;
        if let Some(controls) = &self.controls {
            controls.accept(write, lookup_error).await
        } else if let Some(error) = lookup_error {
            self.inbox.recover_receipt(&write).await?.ok_or(error)
        } else {
            self.inbox.accept(write).await
        }
    }
}

#[cfg(test)]
mod preauth_tests;

fn header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    required: bool,
    max: usize,
) -> Result<Option<&'a str>, CallbackError> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|v| v.to_str().map_err(|_| CallbackError::Authentication))
        .transpose()?;
    if values.next().is_some()
        || (required && value.is_none())
        || value.is_some_and(|s| s.is_empty() || s.len() > max)
    {
        return Err(CallbackError::Authentication);
    }
    Ok(value)
}

fn parse_headers(headers: &HeaderMap) -> Result<CallbackHeaders<'_>, CallbackError> {
    if header(headers, "content-type", true, 64)? != Some("application/json") {
        return Err(CallbackError::Payload);
    }
    Ok(CallbackHeaders {
        callback_token: header(headers, "x-nyxid-callback-token", true, 16384)?
            .ok_or(CallbackError::Authentication)?,
        user_token: header(headers, "x-nyxid-user-token", false, 16384)?,
        message_id: header(headers, "x-nyxid-message-id", true, 256)?
            .ok_or(CallbackError::Authentication)?,
        platform: header(headers, "x-nyxid-platform", true, 256)?
            .ok_or(CallbackError::Authentication)?,
        timestamp: header(headers, "x-nyxid-timestamp", true, 64)?
            .ok_or(CallbackError::Authentication)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_sensitive_headers_and_missing_attestation_fail_closed() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert(
            "x-nyxid-signature",
            "legacy-hmac-is-not-enough".parse().unwrap(),
        );
        assert!(parse_headers(&headers).is_err());
        for (name, value) in [
            ("x-nyxid-callback-token", "signed"),
            ("x-nyxid-message-id", "message"),
            ("x-nyxid-platform", "telegram"),
            ("x-nyxid-timestamp", "2026-09-15T00:00:00Z"),
        ] {
            headers.insert(name, value.parse().unwrap());
        }
        assert!(parse_headers(&headers).is_ok());
        headers.append("x-nyxid-callback-token", "second".parse().unwrap());
        assert!(parse_headers(&headers).is_err());
    }
}
