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
                if current
                    .renew_receiver(&endpoint, &authority, &identity)
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }))))
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
                .map_err(|_| OwnershipFailure::new(phase, Reason::TransportUnavailable))?;
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
            // A dropped link must not discard custody of a receiver that may
            // still hold this lease. A native revoke uses independent state
            // bound to the captured incarnation; its open fences refuse a
            // replaced Pod or container before anything is sent.
            transport.state = match self.state.upgrade() {
                Some(state) => state,
                None => super::native_pool::revoke_state(&self.endpoint)
                    .map(|state| Arc::new(Mutex::new(state)))
                    .ok_or_else(|| {
                        OwnershipFailure::new(Phase::Revoke, Reason::TransportUnavailable)
                    })?,
            };
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
mod tests {
    use super::super::tests::{json_response, listener, read_request};
    use super::*;
    use crate::owner_protocol::{scope, test_support::TestAuthority, ReceiverRevocation};
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn first_revocation_failure_is_classified_and_success_has_no_failure() {
        for refuse in [false, true] {
            let listener = listener().await;
            let address = listener.local_addr().unwrap();
            let endpoint =
                RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}"))
                    .unwrap();
            let transport = ProviderHostedControlTransport::for_retained_opensandbox(
                SandboxClient::new("http://unused.invalid", "fixture-key"),
            )
            .unwrap();
            let authority = TestAuthority::new(1);
            let owner = authority.owner();
            let server = tokio::spawn(async move {
                for action in ["challenge", "confirm", "revoke"] {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let request = String::from_utf8(read_request(&mut stream).await).unwrap();
                    assert!(request.contains(&format!("/manager-ownership/{action}")));
                    let bytes = if action == "revoke" && refuse {
                        b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
                            .to_vec()
                    } else if action == "challenge" {
                        json_response(
                            &serde_json::json!({"protocol":1,"nonce":uuid::Uuid::new_v4(),"valid_for_ms":10000}),
                        )
                    } else {
                        json_response(&OwnerAcknowledgement {
                            owner: owner.clone(),
                            protocol: 1,
                        })
                    };
                    stream.write_all(&bytes).await.unwrap();
                }
            });
            let lease = scope(authority.clone(), transport.receiver_lease(&endpoint))
                .await
                .unwrap();
            assert!(authority.failure.get().is_none());
            let receiver: Arc<dyn ReceiverRevocation> =
                authority.receivers.lock().unwrap()[0].clone();
            assert_eq!(receiver.revoke().await.is_err(), refuse);
            server.await.unwrap();
            assert_eq!(
                authority.failure.get().copied(),
                refuse.then(|| OwnershipFailure::http(Phase::Revoke, 403))
            );
            drop(lease);
        }
    }
}

#[path = "provider_ownership_retirement.rs"]
mod retirement;
