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
        self.receiver_lease_with_budget(endpoint, None).await
    }

    pub(super) async fn receiver_lease_for_control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &crate::wire::RequestFrame,
    ) -> crate::Result<Option<ReceiverLease>> {
        let deadline = crate::file_receiver_retry::deadline(request)
            .filter(|_| endpoint.retirement_receiver().is_some());
        self.receiver_lease_with_budget(endpoint, deadline).await
    }

    async fn receiver_lease_with_budget(
        &self,
        endpoint: &RuntimeEndpoint,
        deadline: Option<tokio::time::Instant>,
    ) -> crate::Result<Option<ReceiverLease>> {
        let Some(authority) = current_authority() else {
            return Ok(None);
        };
        let mut template = self.clone();
        template.state = Arc::new(Mutex::new(TransportState::default()));
        let identity = format!("{}:{:?}", endpoint.control()?, endpoint.native_binding());
        let lookup_deadline = attempt_deadline(deadline)?;
        let legacy_incarnation = match tokio::time::timeout_at(
            lookup_deadline,
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
        let first_deadline = if deadline.is_some() {
            if tokio::time::Instant::now() >= lookup_deadline {
                return Err(negotiation_budget_expired());
            }
            lookup_deadline
        } else {
            attempt_deadline(None)?
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
        self.negotiate_and_revoke_on_failure(
            endpoint,
            &authority,
            &identity,
            first_deadline,
            deadline,
        )
        .await?;
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
                    .negotiate_and_revoke_on_failure(
                        &endpoint,
                        &authority,
                        &identity,
                        tokio::time::Instant::now() + NEGOTIATION_DEADLINE,
                        None,
                    )
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

    async fn negotiate_and_revoke_on_failure(
        &self,
        endpoint: &RuntimeEndpoint,
        authority: &Arc<dyn TransportAuthority>,
        identity: &str,
        first_deadline: tokio::time::Instant,
        deadline: Option<tokio::time::Instant>,
    ) -> crate::Result<()> {
        let evidence = authority.retry_evidence();
        let _negotiation = evidence.as_ref().map(|evidence| evidence.negotiation());
        let mut failed = match self
            .negotiate_attempt(endpoint, authority, identity, first_deadline, false)
            .await
        {
            Ok(()) => return Ok(()),
            Err(failed) => failed,
        };
        let retry_deadline = deadline
            .filter(|_| {
                failed.retryable && authority.is_current() && crate::file_receiver_retry::take()
            })
            .and_then(|deadline| attempt_deadline(Some(deadline)).ok());
        if let Some(retry_deadline) = retry_deadline {
            let reason = failed.failure.observation.reason.as_str();
            let result = self
                .negotiate_attempt(endpoint, authority, identity, retry_deadline, true)
                .await;
            metrics::counter!(
                "cma_agent_file_receiver_retries_total",
                "reason" => reason,
                "outcome" => if result.is_ok() { "recovered" } else { "failed" },
            )
            .increment(1);
            match result {
                Ok(()) => return Ok(()),
                Err(retry) => failed = retry,
            }
        }
        let failure = failed.failure;
        let receiver = endpoint.native_binding().and_then(|(handle, binding)| {
            crate::owner_protocol::OwnershipReceiver::from_native(handle, binding)
        });
        if let Some(receiver) = receiver.clone() {
            crate::owner_protocol::record_inspection_refusal(
                authority.owner(),
                receiver,
                failure.observation,
            );
        }
        if let Some(evidence) = evidence.as_ref() {
            evidence.record(
                authority.owner(),
                receiver,
                failure.observation,
                failed.challenge_valid,
            );
        }
        authority
            .record_failure_and_revoke(failure.observation)
            .await;
        Err(failure.error)
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
                    let origin = if observed_status.oversized() {
                        crate::owner_protocol::ReceiverSettlementFailure::Invalid
                    } else {
                        observed_status.settlement_origin().unwrap_or(
                            crate::owner_protocol::ReceiverSettlementFailure::Unclassified,
                        )
                    };
                    failure.with_error(error).with_settlement_origin(origin)
                })?;
            let status = reply.status.as_u16();
            observed_status.record_status(status);
            if !reply.status.is_success() {
                self.record_failed_challenge(endpoint, owner, phase, reply.status);
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
                .map_err(|error| {
                    let origin = if error.is_connect() || error.is_timeout() {
                        crate::owner_protocol::ReceiverSettlementFailure::Unavailable
                    } else {
                        crate::owner_protocol::ReceiverSettlementFailure::Unclassified
                    };
                    OwnershipExchangeFailure::from(OwnershipFailure::new(
                        phase,
                        Reason::TransportUnavailable,
                    ))
                    .with_settlement_origin(origin)
                })?;
            let status = reply.status().as_u16();
            observed_status.record_status(status);
            if !reply.status().is_success() {
                self.record_failed_challenge(endpoint, owner, phase, reply.status());
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

#[path = "provider_ownership_settlement.rs"]
mod settlement;

fn attempt_deadline(window: Option<tokio::time::Instant>) -> crate::Result<tokio::time::Instant> {
    let now = tokio::time::Instant::now();
    match window {
        Some(deadline) if deadline <= now => Err(negotiation_budget_expired()),
        Some(deadline) => Ok(deadline.min(now + NEGOTIATION_DEADLINE)),
        None => Ok(now + NEGOTIATION_DEADLINE),
    }
}

fn negotiation_budget_expired() -> crate::AdapterError {
    crate::AdapterError::Unavailable("file read receiver negotiation budget expired".into())
}

#[path = "provider_ownership_attempt.rs"]
mod attempt;

#[cfg(test)]
#[path = "provider_ownership_settlement_tests.rs"]
mod settlement_tests;
#[cfg(test)]
#[path = "provider_ownership_revocation_tests.rs"]
mod tests;

#[path = "provider_ownership_retirement.rs"]
mod retirement;
