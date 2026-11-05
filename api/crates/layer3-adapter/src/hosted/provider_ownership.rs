//! Receiving leases are renewed only after an unpredictable receiver challenge.
use super::*;
use crate::owner_protocol::{
    current_authority, OwnerAcknowledgement, OwnerChallenge, OwnerConfirmation, TransportAuthority,
    TransportOwner, OWNER_PROTOCOL_VERSION, RECEIVER_LEASE_MS,
};
use serde::{de::DeserializeOwned, Serialize};

const NEGOTIATION_DEADLINE: Duration = Duration::from_secs(8);
const RENEW_INTERVAL: Duration = Duration::from_secs(5);
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
        let url = endpoint.ownership(&owner.agent_id, action)?;
        let bytes = if endpoint.native_binding().is_some() {
            let body = serde_json::to_vec(request).map_err(|_| rejected())?;
            let reply = self
                .native_http(endpoint, http::Method::POST, url, body, RESPONSE_LIMIT)
                .await?;
            if !reply.status.is_success() {
                return Err(rejected());
            }
            reply.recycle().await?
        } else {
            let (name, token) = self.auth_header(endpoint);
            let mut reply = self
                .http
                .post(url)
                .header(name, token)
                .json(request)
                .send()
                .await
                .map_err(|_| rejected())?;
            if !reply.status().is_success() {
                return Err(rejected());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = reply.chunk().await.map_err(|_| rejected())? {
                if chunk.len() > RESPONSE_LIMIT.saturating_sub(bytes.len()) {
                    return Err(rejected());
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        };
        serde_json::from_slice(&bytes).map_err(|_| rejected())
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
        let mut transport = self.template.clone();
        transport.state = self.state.upgrade().ok_or_else(rejected)?;
        let reply: OwnerAcknowledgement = tokio::time::timeout(
            NEGOTIATION_DEADLINE,
            transport.owner_exchange(&self.endpoint, &self.owner, "revoke", &self.owner),
        )
        .await
        .map_err(|_| rejected())??;
        if reply.protocol != OWNER_PROTOCOL_VERSION || reply.owner != self.owner {
            return Err(rejected());
        }
        Ok(())
    }
}
