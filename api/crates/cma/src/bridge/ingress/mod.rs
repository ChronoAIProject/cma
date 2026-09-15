//! Dedicated signed-callback adapter. Production routes stay closed while
//! selected-key custody is unavailable. No callback claims can mint a binding.
use std::{sync::Arc, time::Duration};

use axum::http::HeaderMap;
use layer2_core::bridge::ingress::{
    CallbackBinding, CallbackError, CallbackHeaders, CallbackInboxStore, CallbackTokenPolicy,
    CallbackVerifier, InboxReceipt,
};
use layer2_core::bridge::{BridgeConnectionValidator, BridgeId, BridgeStore};
use tokio::sync::Semaphore;

mod capabilities;
#[cfg(test)]
pub(crate) use capabilities::NyxidCapabilities;
mod configured;
pub(crate) mod http;
pub(crate) use configured::configured;
mod keys;
pub(crate) use keys::PinnedCallbackKeys;

pub(crate) struct CallbackIngress {
    bridges: Arc<dyn BridgeStore>,
    connections: Arc<dyn BridgeConnectionValidator>,
    inbox: Arc<dyn CallbackInboxStore>,
    keys: PinnedCallbackKeys,
    verifier: CallbackVerifier,
    callback_root: String,
    workers: Semaphore,
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
        })
    }

    /// The transport must also limit the streaming body to 1 MiB before buffering.
    /// Only durable success may map to HTTP 202. Timeout means acceptance is
    /// unknown; a valid retry may reconcile a committed receipt.
    pub(crate) async fn accept(
        &self,
        id: &BridgeId,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<InboxReceipt, CallbackError> {
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
    ) -> Result<InboxReceipt, CallbackError> {
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
        }
        .classify_capabilities(body, &capabilities::NyxidCapabilities)?;
        // Only cryptographically authenticated and bounded callbacks may cause
        // selected-key custody or owner/provider inventory IO. The signed
        // snapshot alone still cannot authorize durable acceptance.
        let proof = self
            .connections
            .validate_connection(&bridge.owner, config)
            .await
            .map_err(|_| CallbackError::BindingUnavailable)?;
        let write = signed.bind(CallbackBinding::new(bridge, proof)?)?;
        self.inbox.accept(write).await
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
