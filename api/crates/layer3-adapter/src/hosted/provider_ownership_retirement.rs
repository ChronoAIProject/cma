use super::*;

impl ProviderHostedControlTransport {
    pub(in crate::hosted) async fn challenge_native_receiver(
        &self,
        handle: &RuntimeHandle,
        binding: &crate::agent_sandbox::PodBinding,
        owner: &TransportOwner,
    ) -> crate::Result<bool> {
        owner.validate()?;
        let RuntimeDiscovery::AgentSandbox(client) = &self.client else {
            return Err(AdapterError::Unsupported(
                "native receiver is unavailable".into(),
            ));
        };
        if client.pod_binding(handle).await? != *binding {
            return Err(AdapterError::Rejected("retirement receiver changed".into()));
        }
        let address = std::net::SocketAddr::new(binding.pod_ip, SUPERVISOR_PORT);
        let mut endpoint =
            RuntimeEndpoint::parse("http://runtime.invalid", &format!("http://{address}"))?
                .with_control_token(client.control_token(&binding.sandbox_name));
        if self.native_transport != NativeRuntimeTransport::Direct {
            let (identity, _) = self.native_endpoint_state(handle).await;
            endpoint = endpoint
                .with_native_binding(handle.clone(), binding.clone())
                .with_native_identity(identity);
        }
        let result = self.challenge_only(&endpoint, owner).await;
        if client.pod_binding(handle).await? != *binding {
            return Err(AdapterError::Rejected("retirement receiver changed".into()));
        }
        result
    }

    pub(in crate::hosted) async fn challenge_only(
        &self,
        endpoint: &RuntimeEndpoint,
        owner: &TransportOwner,
    ) -> crate::Result<bool> {
        let status = Arc::new(NativeHttpObservation::default());
        let response = tokio::time::timeout(
            NEGOTIATION_DEADLINE,
            self.owner_exchange::<_, OwnerChallenge>(
                endpoint,
                owner,
                Phase::Challenge,
                owner,
                &status,
            ),
        )
        .await
        .map_err(|_| AdapterError::Unavailable("retirement challenge timed out".into()))?;
        match response {
            Ok((challenge, _))
                if challenge.protocol == OWNER_PROTOCOL_VERSION
                    && challenge.valid_for_ms == RECEIVER_LEASE_MS
                    && uuid::Uuid::parse_str(&challenge.nonce).is_ok() =>
            {
                Ok(false)
            }
            Ok(_) => Err(AdapterError::Rejected(
                "retirement challenge reply is invalid".into(),
            )),
            Err(failure)
                if failure
                    .observation
                    .http_status
                    .and_then(|status| http::StatusCode::from_u16(status).ok())
                    .is_some_and(|status| {
                        crate::receiver_failure::unavailable_status(status)
                            && failure.observation
                                == OwnershipFailure::http(Phase::Challenge, status.as_u16())
                    }) =>
            {
                Ok(true)
            }
            Err(failure) => Err(failure.error),
        }
    }

    pub(super) fn record_failed_challenge(
        &self,
        endpoint: &RuntimeEndpoint,
        owner: &TransportOwner,
        phase: Phase,
        status: http::StatusCode,
    ) {
        if phase != Phase::Challenge || !crate::receiver_failure::unavailable_status(status) {
            return;
        }
        crate::receiver_failure::record(|| {
            let (runtime, pod) = endpoint.retirement_receiver()?;
            Some(crate::receiver_failure::RetirementReceiver {
                runtime,
                pod,
                owner: owner.clone(),
            })
        });
    }
}
