//! Receiving leases are renewed only after an unpredictable receiver challenge.
use super::*;
use crate::owner_protocol::{
    current_authority, OwnerAcknowledgement, OwnerChallenge, OwnerConfirmation, TransportAuthority,
    TransportOwner, OWNER_PROTOCOL_VERSION, RECEIVER_LEASE_MS,
};
use serde::{de::DeserializeOwned, Serialize};

// Two bounded negotiations plus the intervening sleep fit within the receiver
// lifetime, including a slow initial confirmation. Never extend a challenge.
const NEGOTIATION_DEADLINE: Duration = Duration::from_secs(3);
const RENEW_INTERVAL: Duration = Duration::from_secs(3);
const RESPONSE_LIMIT: usize = 4_096;

pub(super) struct ReceiverLease(tokio::task::JoinHandle<()>);

impl Drop for ReceiverLease {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl ProviderHostedControlTransport {
    pub(super) async fn receiver_lease(
        &self,
        endpoint: &RuntimeEndpoint,
    ) -> crate::Result<Option<ReceiverLease>> {
        let Some(authority) = current_authority() else {
            return Ok(None);
        };
        let mut template = self.clone();
        template.state = Arc::new(Mutex::new(TransportState::default()));
        authority.register_receiver(Arc::new(ReceiverFence {
            identity: format!("{}:{:?}", endpoint.control()?, endpoint.native_binding()),
            template,
            state: Arc::downgrade(&self.state),
            endpoint: endpoint.clone(),
            owner: authority.owner(),
        }))?;
        self.renew_receiver(endpoint, &authority).await?;
        // A live link owns this task; the task must not own the link's state.
        let state = Arc::downgrade(&self.state);
        let mut transport = self.clone();
        transport.state = Arc::new(Mutex::new(TransportState::default()));
        let endpoint = endpoint.clone();
        Ok(Some(ReceiverLease(tokio::spawn(async move {
            loop {
                tokio::select! { biased;
                    _ = authority.revoked() => return,
                    _ = tokio::time::sleep(RENEW_INTERVAL) => (),
                }
                let Some(state) = state.upgrade() else { return };
                let mut current = transport.clone();
                current.state = state;
                if current.renew_receiver(&endpoint, &authority).await.is_err() {
                    authority.revoke();
                    return;
                }
            }
        }))))
    }

    async fn renew_receiver(
        &self,
        endpoint: &RuntimeEndpoint,
        authority: &Arc<dyn TransportAuthority>,
    ) -> crate::Result<()> {
        let operation = async {
            authority.check().await?;
            let owner = authority.owner();
            let challenge: OwnerChallenge = self
                .owner_exchange(endpoint, &owner, "challenge", &owner)
                .await?;
            if challenge.protocol != OWNER_PROTOCOL_VERSION
                || challenge.valid_for_ms != RECEIVER_LEASE_MS
                || uuid::Uuid::parse_str(&challenge.nonce).is_err()
            {
                return Err(rejected());
            }
            // Causal ordering is essential. Never confirm with a cached renewal.
            authority.renew_for_challenge().await?;
            let request = OwnerConfirmation {
                owner: owner.clone(),
                nonce: challenge.nonce,
            };
            let confirmed: OwnerAcknowledgement = self
                .owner_exchange(endpoint, &owner, "confirm", &request)
                .await?;
            if confirmed.protocol != OWNER_PROTOCOL_VERSION || confirmed.owner != owner {
                return Err(rejected());
            }
            authority.check().await
        };
        let result = tokio::select! { biased;
            _ = authority.revoked() => Err(rejected()),
            result = tokio::time::timeout(NEGOTIATION_DEADLINE, operation) => result.unwrap_or_else(|_| Err(rejected())),
        };
        if result.is_err() {
            authority.revoke();
        }
        result
    }

    async fn owner_exchange<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        endpoint: &RuntimeEndpoint,
        owner: &TransportOwner,
        action: &str,
        request: &T,
    ) -> crate::Result<R> {
        self.owner_exchange_checked(endpoint, owner, action, request)
            .await
            .map_err(|failure| match failure {
                // Preserve the existing challenge/confirmation error contract.
                RevocationFailure::NativeTransport(error) => error,
                _ => rejected(),
            })
    }

    async fn owner_exchange_checked<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        endpoint: &RuntimeEndpoint,
        owner: &TransportOwner,
        action: &str,
        request: &T,
    ) -> Result<R, RevocationFailure> {
        let url = endpoint
            .ownership(&owner.agent_id, action)
            .map_err(|_| RevocationFailure::Transport)?;
        let bytes = if endpoint.native_binding().is_some() {
            let body =
                serde_json::to_vec(request).map_err(|_| RevocationFailure::InvalidResponse)?;
            let reply = self
                .native_http_for(
                    endpoint,
                    http::Method::POST,
                    url,
                    body,
                    RESPONSE_LIMIT,
                    native_pool::NativeHttpLane::Ownership,
                )
                .await
                .map_err(RevocationFailure::NativeTransport)?;
            if !reply.status.is_success() {
                return Err(RevocationFailure::HttpStatus(reply.status.as_u16()));
            }
            reply
                .recycle()
                .await
                .map_err(RevocationFailure::NativeTransport)?
        } else {
            let (name, token) = self.auth_header(endpoint);
            let mut reply = self
                .http
                .post(url)
                .header(name, token)
                .json(request)
                .send()
                .await
                .map_err(|_| RevocationFailure::Transport)?;
            if !reply.status().is_success() {
                return Err(RevocationFailure::HttpStatus(reply.status().as_u16()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = reply
                .chunk()
                .await
                .map_err(|_| RevocationFailure::Transport)?
            {
                if chunk.len() > RESPONSE_LIMIT.saturating_sub(bytes.len()) {
                    return Err(RevocationFailure::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        };
        serde_json::from_slice(&bytes).map_err(|_| RevocationFailure::InvalidResponse)
    }
}

fn rejected() -> AdapterError {
    AdapterError::Rejected("runtime ownership negotiation failed or is unsupported".into())
}

struct ReceiverFence {
    identity: String,
    template: ProviderHostedControlTransport,
    state: std::sync::Weak<Mutex<TransportState>>,
    endpoint: RuntimeEndpoint,
    owner: TransportOwner,
}

#[async_trait::async_trait]
impl crate::owner_protocol::ReceiverRevocation for ReceiverFence {
    fn identity(&self) -> String {
        self.identity.clone()
    }

    async fn revoke(&self) -> crate::Result<()> {
        let result = async {
            let mut transport = self.template.clone();
            transport.state = self
                .state
                .upgrade()
                .ok_or(RevocationFailure::LocalStateLost)?;
            let reply: OwnerAcknowledgement = tokio::time::timeout(
                NEGOTIATION_DEADLINE,
                transport.owner_exchange_checked(
                    &self.endpoint,
                    &self.owner,
                    "revoke",
                    &self.owner,
                ),
            )
            .await
            .map_err(|_| RevocationFailure::Timeout)??;
            if reply.protocol != OWNER_PROTOCOL_VERSION {
                return Err(RevocationFailure::ProtocolMismatch);
            }
            if reply.owner != self.owner {
                return Err(RevocationFailure::OwnerMismatch);
            }
            Ok(())
        }
        .await;
        result.map_err(|failure| {
            // Stable typed facts only: never log credentials, URLs or response
            // bodies. Lost local state is not proof of a remote revocation.
            use sha2::{Digest, Sha256};
            let agent_sha256 = format!("{:x}", Sha256::digest(self.owner.agent_id.as_bytes()));
            tracing::warn!(stage = "receiver_revocation", failure_kind = failure.kind(),
                http_status = failure.http_status(), epoch = self.owner.epoch, %agent_sha256,
                "Receiver revocation was not confirmed");
            AdapterError::Rejected(format!("runtime receiver revocation failed: {failure}"))
        })
    }
}

#[derive(Debug, thiserror::Error)]
enum RevocationFailure {
    #[error("local_state_lost")]
    LocalStateLost,
    #[error("transport")]
    Transport,
    #[error("transport")]
    NativeTransport(AdapterError),
    #[error("http_status({0})")]
    HttpStatus(u16),
    #[error("invalid_response")]
    InvalidResponse,
    #[error("protocol_mismatch")]
    ProtocolMismatch,
    #[error("owner_mismatch")]
    OwnerMismatch,
    #[error("timeout")]
    Timeout,
}

impl RevocationFailure {
    fn kind(&self) -> &'static str {
        match self {
            Self::LocalStateLost => "local_state_lost",
            Self::Transport | Self::NativeTransport(_) => "transport",
            Self::HttpStatus(_) => "http_status",
            Self::InvalidResponse => "invalid_response",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::OwnerMismatch => "owner_mismatch",
            Self::Timeout => "timeout",
        }
    }

    fn http_status(&self) -> Option<u16> {
        match self {
            Self::HttpStatus(status) => Some(*status),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "provider_ownership_revocation_tests.rs"]
mod revocation_tests;
