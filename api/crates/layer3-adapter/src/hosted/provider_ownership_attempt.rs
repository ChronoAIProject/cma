use super::*;

pub(super) struct FailedAttempt {
    pub failure: OwnershipExchangeFailure,
    pub retryable: bool,
    pub challenge_valid: bool,
}

impl ProviderHostedControlTransport {
    pub(super) async fn negotiate_attempt(
        &self,
        endpoint: &RuntimeEndpoint,
        authority: &Arc<dyn TransportAuthority>,
        identity: &str,
        deadline: tokio::time::Instant,
        retry: bool,
    ) -> Result<(), FailedAttempt> {
        let phase = std::sync::Mutex::new(Phase::Challenge);
        let evidence = authority.retry_evidence();
        let receiver = endpoint.native_binding().and_then(|(handle, binding)| {
            crate::owner_protocol::OwnershipReceiver::from_native(handle, binding)
        });
        let challenge_valid = std::sync::atomic::AtomicBool::new(false);
        let challenge_started = std::sync::atomic::AtomicBool::new(false);
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
            if retry {
                self.check_retry_receiver(endpoint).await.map_err(|error| {
                    OwnershipFailure::new(Phase::Challenge, Reason::TransportUnavailable)
                        .with_error(error)
                })?;
            }
            challenge_started.store(true, std::sync::atomic::Ordering::Release);
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
            result = tokio::time::timeout_at(deadline, operation) => result.unwrap_or_else(|_| Err(interrupted(Reason::Deadline))),
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
            return Err(FailedAttempt {
                retryable: challenge_started.load(std::sync::atomic::Ordering::Acquire)
                    && failure.observation.phase == Phase::Challenge
                    && match failure.observation.reason {
                        Reason::Deadline => failure
                            .observation
                            .http_status
                            .is_none_or(|status| (200..300).contains(&status)),
                        Reason::HttpAuthRefused => failure.observation.http_status == Some(403),
                        _ => false,
                    },
                challenge_valid: challenge_valid.load(std::sync::atomic::Ordering::Acquire),
                failure,
            });
        }
        Ok(())
    }

    async fn check_retry_receiver(&self, endpoint: &RuntimeEndpoint) -> crate::Result<()> {
        let Some((handle, expected)) = endpoint.retirement_receiver() else {
            return Err(rejected());
        };
        let fresh = self.endpoint(&handle).await?;
        if fresh.retirement_receiver() != Some((handle, expected))
            || fresh.native_identity() != endpoint.native_identity()
            || fresh.native_binding() != endpoint.native_binding()
            || fresh.control()? != endpoint.control()?
            || fresh.control_token() != endpoint.control_token()
        {
            return Err(crate::AdapterError::Rejected(
                "ownership receiver changed before retry".into(),
            ));
        }
        Ok(())
    }
}
