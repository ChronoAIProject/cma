//! Receiving leases are renewed only after an unpredictable receiver challenge.
use super::native_transport::NativeHttpObservation;
use super::*;
use crate::agent_sandbox::ReceiverIncarnation;
use crate::owner_protocol::{
    current_authority, OwnerAcknowledgement, OwnerChallenge, OwnerConfirmation,
    OwnershipExchangeFailure, OwnershipFailure, OwnershipFailureReason as Reason,
    OwnershipPhase as Phase, ReceiverCleanup, ReceiverConfirmationCustody, TransportAuthority,
    TransportOwner, OWNER_PROTOCOL_VERSION, RECEIVER_LEASE_MS,
};
use serde::{de::DeserializeOwned, Serialize};

// Two bounded negotiations plus the intervening sleep fit within the receiver
// lifetime, including a slow initial confirmation. Never extend a challenge.
const NEGOTIATION_DEADLINE: Duration = Duration::from_secs(3);
const RENEW_INTERVAL: Duration = Duration::from_secs(3);
const RESPONSE_LIMIT: usize = 4_096;

pub(super) struct ReceiverLease {
    renewal: tokio::task::JoinHandle<()>,
    authority: Arc<dyn TransportAuthority>,
    owner: TransportOwner,
    endpoint: RuntimeEndpoint,
    state: std::sync::Weak<Mutex<TransportState>>,
}

/// An Open cannot borrow a lease whose exact authority, link or endpoint changed.
fn rejected() -> AdapterError {
    AdapterError::Rejected("runtime ownership negotiation failed or is unsupported".into())
}

impl ReceiverLease {
    pub(super) async fn check_open(
        &self,
        transport: &ProviderHostedControlTransport,
        endpoint: &RuntimeEndpoint,
        agent: &AgentSlot,
    ) -> crate::Result<()> {
        let authority = current_authority().ok_or_else(rejected)?;
        // This is a borrowed capability for one Open, not an authority cache.
        // Even an equal token from another authority must negotiate for itself.
        if !Arc::ptr_eq(&authority, &self.authority)
            || authority.owner() != self.owner
            || self.owner.agent_id != agent.as_str()
            || self.renewal.is_finished()
            || !self.state.ptr_eq(&Arc::downgrade(&transport.state))
            || self.endpoint.control()? != endpoint.control()?
            || self.endpoint.control_token() != endpoint.control_token()
            || self.endpoint.native_identity() != endpoint.native_identity()
            || self.endpoint.native_binding() != endpoint.native_binding()
        {
            return Err(rejected());
        }
        authority.check().await
    }
}

impl Drop for ReceiverLease {
    fn drop(&mut self) {
        self.renewal.abort();
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
        let identity = format!("{}:{:?}", endpoint.control()?, endpoint.native_binding());
        let legacy_incarnation = match tokio::time::timeout(
            NEGOTIATION_DEADLINE,
            self.legacy_receiver_incarnation(endpoint),
        )
        .await
        {
            Ok(Ok(incarnation)) => incarnation,
            _ => {
                tracing::debug!("Native receiver image evidence could not be captured");
                None
            }
        };
        authority.register_receiver(Arc::new(ReceiverFence {
            identity: identity.clone(),
            template,
            state: Arc::downgrade(&self.state),
            endpoint: endpoint.clone(),
            owner: authority.owner(),
            authority: Arc::downgrade(&authority),
            legacy_incarnation,
        }))?;
        self.renew_receiver(endpoint, &authority, &identity).await?;
        authority.receiver_confirmed(&identity);
        let lease_authority = authority.clone();
        let lease_endpoint = endpoint.clone();
        let owner = authority.owner();
        // A live link owns this task; the task must not own the link's state.
        let state = Arc::downgrade(&self.state);
        let mut transport = self.clone();
        transport.state = Arc::new(Mutex::new(TransportState::default()));
        let endpoint = endpoint.clone();
        let renewal = tokio::spawn(async move {
            loop {
                tokio::select! { biased;
                    _ = authority.revoked() => return,
                    _ = tokio::time::sleep(RENEW_INTERVAL) => (),
                }
                let Some(state) = state.upgrade() else { return };
                let mut current = transport.clone();
                current.state = state;
                if current
                    .renew_receiver(&endpoint, &authority, &identity)
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        Ok(Some(ReceiverLease {
            renewal,
            authority: lease_authority,
            owner,
            endpoint: lease_endpoint,
            state: Arc::downgrade(&self.state),
        }))
    }

    async fn renew_receiver(
        &self,
        endpoint: &RuntimeEndpoint,
        authority: &Arc<dyn TransportAuthority>,
        identity: &str,
    ) -> crate::Result<()> {
        let phase = std::sync::Mutex::new(Phase::Challenge);
        let evidence = authority.retry_evidence();
        let _negotiation = evidence.as_ref().map(|evidence| evidence.negotiation());
        let receiver = endpoint.native_binding().and_then(|(handle, binding)| {
            crate::owner_protocol::OwnershipReceiver::from_native(handle, binding)
        });
        let challenge_valid = std::sync::atomic::AtomicBool::new(false);
        let status = Arc::new(NativeHttpObservation::default());
        let interrupted = |reason| {
            let mut failure = OwnershipFailure::new(*phase.lock().unwrap(), reason);
            failure.http_status = status.status();
            OwnershipExchangeFailure::from(failure)
        };
        let operation = async {
            authority
                .check_for_phase(Phase::Challenge)
                .await
                .map_err(|error| {
                    OwnershipFailure::new(Phase::Challenge, Reason::AuthorityLost).with_error(error)
                })?;
            let owner = authority.owner();
            let (challenge, challenge_status): (OwnerChallenge, _) = self
                .owner_exchange(endpoint, &owner, Phase::Challenge, &owner, &status)
                .await?;
            authority.receiver_observed(identity);
            challenge_valid.store(
                uuid::Uuid::parse_str(&challenge.nonce).is_ok(),
                std::sync::atomic::Ordering::Release,
            );
            let reason = if challenge.protocol != OWNER_PROTOCOL_VERSION {
                Some(Reason::ProtocolMismatch)
            } else if challenge.valid_for_ms != RECEIVER_LEASE_MS {
                Some(Reason::LeaseMismatch)
            } else if uuid::Uuid::parse_str(&challenge.nonce).is_err() {
                Some(Reason::MalformedReply)
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(OwnershipFailure {
                    phase: Phase::Challenge,
                    reason,
                    http_status: Some(challenge_status),
                    protocol: Some(challenge.protocol),
                    lease_ms: Some(challenge.valid_for_ms),
                }
                .into());
            }
            // Causal ordering is essential. Never confirm with a cached renewal.
            *phase.lock().unwrap() = Phase::Renew;
            status.clear();
            authority.renew_for_challenge().await.map_err(|error| {
                OwnershipFailure::new(Phase::Renew, Reason::AuthorityLost).with_error(error)
            })?;
            let request = OwnerConfirmation {
                owner: owner.clone(),
                nonce: challenge.nonce,
            };
            *phase.lock().unwrap() = Phase::Confirm;
            authority
                .begin_receiver_confirmation(identity)
                .map_err(|error| {
                    OwnershipFailure::new(Phase::Confirm, Reason::AuthorityLost).with_error(error)
                })?;
            if let Some(evidence) = &evidence {
                evidence.confirmation(receiver.as_ref(), false);
            }
            let (confirmed, status): (OwnerAcknowledgement, _) = self
                .owner_exchange(endpoint, &owner, Phase::Confirm, &request, &status)
                .await?;
            validate_acknowledgement(&confirmed, &owner, Phase::Confirm, status)?;
            if let Some(evidence) = &evidence {
                evidence.confirmation(receiver.as_ref(), true);
            }
            authority
                .check_for_phase(Phase::Confirm)
                .await
                .map_err(|error| {
                    OwnershipFailure::new(Phase::Confirm, Reason::AuthorityLost).with_error(error)
                })
        };
        let result = tokio::select! { biased;
            _ = authority.revoked() => Err(interrupted(Reason::AuthorityLost)),
            result = tokio::time::timeout(NEGOTIATION_DEADLINE, operation) => result.unwrap_or_else(|_| Err(interrupted(Reason::Deadline))),
        };
        if let Err(failure) = result {
            if failure.observation.phase == Phase::Challenge
                && failure
                    .observation
                    .http_status
                    .is_some_and(|status| (200..300).contains(&status))
            {
                authority.receiver_observed(identity);
            }
            if let Some(evidence) = &evidence {
                evidence.record(
                    authority.owner(),
                    receiver,
                    failure.observation,
                    challenge_valid.load(std::sync::atomic::Ordering::Acquire),
                );
            }
            authority
                .record_failure_and_revoke(failure.observation)
                .await;
            return Err(failure.error);
        }
        Ok(())
    }

    async fn owner_exchange<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        endpoint: &RuntimeEndpoint,
        owner: &TransportOwner,
        phase: Phase,
        request: &T,
        observed_status: &Arc<NativeHttpObservation>,
    ) -> Result<(R, u16), OwnershipExchangeFailure> {
        let unavailable =
            |error| OwnershipFailure::new(phase, Reason::TransportUnavailable).with_error(error);
        let url = endpoint
            .ownership(&owner.agent_id, phase.as_str())
            .map_err(unavailable)?;
        let (bytes, status) = if endpoint.native_binding().is_some() {
            let body = serde_json::to_vec(request)
                .map_err(|_| OwnershipFailure::new(phase, Reason::MalformedReply))?;
            let reply = observed_status
                .observe(self.native_http_for(
                    endpoint,
                    http::Method::POST,
                    url,
                    body,
                    RESPONSE_LIMIT,
                    (
                        native_pool::NativeHttpLane::Ownership,
                        RuntimeWorkloadClass::Continuity,
                    ),
                ))
                .await
                .map_err(|error| {
                    let reason = if observed_status.oversized() {
                        Reason::MalformedReply
                    } else {
                        Reason::TransportUnavailable
                    };
                    let mut failure = OwnershipFailure::new(phase, reason);
                    failure.http_status = observed_status.status();
                    failure.with_error(error)
                })?;
            let status = reply.status.as_u16();
            observed_status.record_status(status);
            if !reply.status.is_success() {
                return Err(OwnershipFailure::http(phase, status).into());
            }
            (
                reply.recycle().await.map_err(|error| {
                    OwnershipFailure::new(phase, Reason::TransportUnavailable)
                        .with_status(status)
                        .with_error(error)
                })?,
                status,
            )
        } else {
            let (name, token) = self.auth_header(endpoint);
            let mut reply = self
                .http
                .post(url)
                .header(name, token)
                .json(request)
                .send()
                .await
                .map_err(|_| OwnershipFailure::new(phase, Reason::TransportUnavailable))?;
            let status = reply.status().as_u16();
            observed_status.record_status(status);
            if !reply.status().is_success() {
                return Err(OwnershipFailure::http(phase, status).into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = reply.chunk().await.map_err(|_| {
                OwnershipFailure::new(phase, Reason::TransportUnavailable).with_status(status)
            })? {
                if chunk.len() > RESPONSE_LIMIT.saturating_sub(bytes.len()) {
                    return Err(OwnershipFailure::new(phase, Reason::MalformedReply)
                        .with_status(status)
                        .into());
                }
                bytes.extend_from_slice(&chunk);
            }
            (bytes, status)
        };
        serde_json::from_slice(&bytes)
            .map(|reply| (reply, status))
            .map_err(|_| {
                // A partial projection is diagnostic only: typed decoding must still
                // reject duplicate/unknown fields and malformed ownership identities.
                let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
                let protocol = value
                    .as_ref()
                    .and_then(|value| value.get("protocol"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok());
                let lease_ms = value
                    .as_ref()
                    .and_then(|value| value.get("valid_for_ms"))
                    .and_then(serde_json::Value::as_u64);
                OwnershipFailure {
                    phase,
                    reason: Reason::MalformedReply,
                    http_status: Some(status),
                    protocol,
                    lease_ms,
                }
                .into()
            })
    }
}

fn validate_acknowledgement(
    reply: &OwnerAcknowledgement,
    owner: &TransportOwner,
    phase: Phase,
    status: u16,
) -> Result<(), OwnershipFailure> {
    let reason = if reply.protocol != OWNER_PROTOCOL_VERSION {
        Reason::ProtocolMismatch
    } else if reply.owner != *owner {
        Reason::AuthorityLost
    } else {
        return Ok(());
    };
    Err(OwnershipFailure {
        phase,
        reason,
        http_status: Some(status),
        protocol: Some(reply.protocol),
        lease_ms: None,
    })
}

struct ReceiverFence {
    identity: String,
    template: ProviderHostedControlTransport,
    state: std::sync::Weak<Mutex<TransportState>>,
    endpoint: RuntimeEndpoint,
    owner: TransportOwner,
    authority: std::sync::Weak<dyn TransportAuthority>,
    legacy_incarnation: Option<ReceiverIncarnation>,
}

#[async_trait::async_trait]
impl crate::owner_protocol::ReceiverRevocation for ReceiverFence {
    fn identity(&self) -> String {
        self.identity.clone()
    }

    async fn revoke(&self) -> crate::Result<()> {
        self.settle(ReceiverConfirmationCustody::PossiblyDelivered)
            .await?;
        Ok(())
    }

    async fn settle(&self, custody: ReceiverConfirmationCustody) -> crate::Result<ReceiverCleanup> {
        let status = Arc::new(NativeHttpObservation::default());
        let operation = async {
            let mut transport = self.template.clone();
            transport.state = self.state.upgrade().ok_or_else(|| {
                OwnershipFailure::new(Phase::Revoke, Reason::TransportUnavailable)
            })?;
            let result: Result<(OwnerAcknowledgement, _), _> = transport
                .owner_exchange(
                    &self.endpoint,
                    &self.owner,
                    Phase::Revoke,
                    &self.owner,
                    &status,
                )
                .await;
            match result {
                Ok((reply, status)) => {
                    validate_acknowledgement(&reply, &self.owner, Phase::Revoke, status)?;
                    Ok(ReceiverCleanup::Acknowledged)
                }
                Err(failure) => {
                    if custody == ReceiverConfirmationCustody::NeverSent
                        && failure.observation.reason == Reason::MissingRoute
                        && failure.observation.http_status == Some(404)
                        && self.legacy_incarnation.is_some()
                        && matches!(
                            transport.legacy_receiver_incarnation(&self.endpoint).await,
                            Ok(current) if current == self.legacy_incarnation
                        )
                    {
                        use sha2::{Digest, Sha256};
                        tracing::info!(
                            stage = "revoke",
                            http_status = 404_u16,
                            outcome = "no_receiver_known_incompatible",
                            receiver_sha256 = %format!("{:x}", Sha256::digest(self.identity.as_bytes())),
                            "Native receiver cleanup obligation settled"
                        );
                        return Ok(ReceiverCleanup::NoReceiver);
                    }
                    Err(failure)
                }
            }
        };
        let result = tokio::time::timeout(NEGOTIATION_DEADLINE, operation)
            .await
            .unwrap_or_else(|_| {
                let mut failure = OwnershipFailure::new(Phase::Revoke, Reason::Deadline);
                failure.http_status = status.status();
                Err(failure.into())
            });
        match result {
            Ok(outcome) => Ok(outcome),
            Err(failure) => {
                if let Some(authority) = self.authority.upgrade() {
                    authority.record_failure(failure.observation).await;
                }
                Err(failure.error)
            }
        }
    }
}

#[cfg(test)]
#[path = "provider_ownership_revocation_tests.rs"]
mod tests;
