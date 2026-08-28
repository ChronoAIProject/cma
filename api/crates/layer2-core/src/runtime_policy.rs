//! Installing opaque authenticated policy on one exact live runtime.

use crate::{CapabilityRefusal, CoreError, SessionId, SessionService};

/// Exact non-secret policy state observed under one session lifecycle fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePolicyObservation {
    pub runtime_audience: String,
    pub required: bool,
    pub supported: bool,
    pub installed: bool,
    pub revoked: bool,
    pub launch_id: Option<String>,
    pub generation: Option<u64>,
    pub fence: Option<u64>,
    pub expires_at_epoch_seconds: Option<u64>,
    pub last_refusal: Option<CapabilityRefusal>,
}

/// Safe result of installing policy and proving the exact runtime's gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGatewayReachabilityReceipt {
    pub runtime_audience: String,
    pub policy: layer3_adapter::RuntimePolicyReceipt,
    pub probe_version: u32,
}

/// Closed failure vocabulary for the pre-prompt runtime proof.
///
/// Adapter diagnostics are intentionally not retained: they can contain a
/// request URL, command path, or provider material and none is needed to make
/// the Launch retry/security decision.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeGatewayReachabilityError {
    #[error("runtime command sandbox initialization failed")]
    CommandSandboxInitialization,
    #[error("runtime gateway network initialization failed")]
    GatewayNetworkInitialization,
    #[error("runtime signed policy denied the gateway probe")]
    PolicyDenied,
    #[error("provider denied the gateway probe")]
    ProviderDenied,
    #[error("runtime gateway broker is temporarily unavailable")]
    TransientBrokerFailure,
    #[error("runtime is unavailable")]
    RuntimeUnavailable,
    #[error("runtime gateway probe protocol is unsupported")]
    ProtocolUnsupported,
    #[error("runtime gateway probe protocol was violated")]
    ProtocolViolation,
    #[error("runtime credential authority changed before policy installation")]
    CredentialAuthorityDrift,
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl SessionService {
    /// Opaque identity of the exact runtime currently serving this session.
    pub async fn runtime_policy_audience(&self, id: &SessionId) -> Result<String, CoreError> {
        let _lifecycle = self.lock_lifecycle(id).await;
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or_else(|| {
            CoreError::RuntimeUnavailable("the session has no live runtime entry".into())
        })?;
        if entry.trigger_origin.is_none() {
            return Err(CoreError::InvalidState);
        }
        entry
            .runtime
            .as_ref()
            .map(|runtime| runtime.0.clone())
            .ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no current runtime".into())
            })
    }

    /// Observe the authenticated-policy tuple of the exact current runtime.
    ///
    /// The lifecycle guard is held across handle selection and the readiness
    /// request. A successful result can therefore prove that a supervisor
    /// restarted within an otherwise unchanged provider runtime.
    pub async fn runtime_policy_observation(
        &self,
        id: &SessionId,
    ) -> Result<RuntimePolicyObservation, CoreError> {
        let _lifecycle = self.lock_lifecycle(id).await;
        let (runtime, kernel) = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no live runtime entry".into())
            })?;
            if entry.trigger_origin.is_none() {
                return Err(CoreError::InvalidState);
            }
            let runtime = entry.runtime.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no current runtime".into())
            })?;
            (runtime, entry.session.kernel.clone())
        };
        let adapter = self.adapters.get(&kernel).ok_or_else(|| {
            CoreError::CapabilityUnsupported("authenticated_runtime_policy_observation".into())
        })?;
        let observation = adapter
            .observe_runtime_policy(&runtime)
            .await
            .map_err(map_policy_observation_adapter_error)?;
        Ok(RuntimePolicyObservation {
            runtime_audience: runtime.0,
            required: observation.required,
            supported: observation.supported,
            installed: observation.installed,
            revoked: observation.revoked,
            launch_id: observation.launch_id,
            generation: observation.generation,
            fence: observation.fence,
            expires_at_epoch_seconds: observation.expires_at_epoch_seconds,
            last_refusal: observation.last_refusal.map(map_runtime_policy_refusal),
        })
    }

    /// Build policy against the current runtime audience, deliver it while
    /// lifecycle replacement is fenced, and return the exact acknowledgement.
    ///
    /// The closure keeps provider-shaped signing outside Layer 2. The core
    /// owns only the ordering invariant: the runtime named in the signed data
    /// is the same runtime that receives it, for the whole operation.
    pub async fn install_runtime_policy<F>(
        &self,
        id: &SessionId,
        build: F,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError>
    where
        F: FnOnce(&str) -> Result<String, CoreError> + Send,
    {
        let _lifecycle = self.lock_lifecycle(id).await;
        let (runtime, kernel) = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no live runtime entry".into())
            })?;
            if entry.trigger_origin.is_none() {
                return Err(CoreError::InvalidState);
            }
            let runtime = entry.runtime.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no current runtime".into())
            })?;
            (runtime, entry.session.kernel.clone())
        };
        let adapter = self.adapters.get(&kernel).ok_or_else(|| {
            CoreError::CapabilityUnsupported("authenticated_runtime_policy".into())
        })?;
        let envelope = build(&runtime.0)?;
        let receipt = adapter
            .install_runtime_policy(&runtime, &envelope)
            .await
            .map_err(|error| match error {
                layer3_adapter::AdapterError::Unavailable(detail) => {
                    CoreError::RuntimeUnavailable(detail)
                }
                layer3_adapter::AdapterError::Rejected(detail)
                | layer3_adapter::AdapterError::AtCapacity(detail) => {
                    CoreError::RuntimeRefused(detail)
                }
                layer3_adapter::AdapterError::Unsupported(_) => {
                    CoreError::CapabilityUnsupported("authenticated_runtime_policy".into())
                }
            })?;
        Ok((runtime.0, receipt))
    }

    /// Install policy and prove its one allowed gateway read under one exact
    /// lifecycle fence and against one exact runtime handle.
    ///
    /// Splitting these into two public calls would leave a replacement window:
    /// policy could be acknowledged by H1 and the probe could run on H2. The
    /// lifecycle guard intentionally spans signing, both adapter calls, and
    /// receipt validation so success always describes one runtime generation.
    pub async fn install_runtime_policy_and_probe<F>(
        &self,
        id: &SessionId,
        request: layer3_adapter::RuntimeGatewayProbeRequest,
        build: F,
    ) -> Result<RuntimeGatewayReachabilityReceipt, RuntimeGatewayReachabilityError>
    where
        F: FnOnce(&str) -> Result<String, CoreError> + Send,
    {
        self.install_runtime_policy_and_probe_inner(id, request, None, build)
            .await
    }

    /// Trigger-only policy installation bound to the exact acknowledged
    /// session credential admitted by the Launch worker.
    pub async fn install_runtime_policy_and_probe_with_credential_authority<F>(
        &self,
        id: &SessionId,
        request: layer3_adapter::RuntimeGatewayProbeRequest,
        expected: &crate::SessionCredentialAuthority,
        build: F,
    ) -> Result<RuntimeGatewayReachabilityReceipt, RuntimeGatewayReachabilityError>
    where
        F: FnOnce(&str) -> Result<String, CoreError> + Send,
    {
        self.install_runtime_policy_and_probe_inner(id, request, Some(expected), build)
            .await
    }

    async fn install_runtime_policy_and_probe_inner<F>(
        &self,
        id: &SessionId,
        request: layer3_adapter::RuntimeGatewayProbeRequest,
        expected: Option<&crate::SessionCredentialAuthority>,
        build: F,
    ) -> Result<RuntimeGatewayReachabilityReceipt, RuntimeGatewayReachabilityError>
    where
        F: FnOnce(&str) -> Result<String, CoreError> + Send,
    {
        if request.version != layer3_adapter::RUNTIME_GATEWAY_PROBE_VERSION
            || request.resource_id == 0
        {
            return Err(RuntimeGatewayReachabilityError::ProtocolViolation);
        }
        let _lifecycle = self.lock_lifecycle(id).await;
        let (runtime, kernel, credential_acknowledged) = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(id)
                .ok_or(RuntimeGatewayReachabilityError::RuntimeUnavailable)?;
            let origin = entry
                .trigger_origin
                .as_ref()
                .ok_or(RuntimeGatewayReachabilityError::ProtocolViolation)?;
            if origin.github_service_id != request.service_id
                || origin.github_binding().ok()
                    != Some((
                        request.service_id.as_str(),
                        request.catalog_service_id.as_str(),
                        request.user_service_slug.as_str(),
                    ))
            {
                return Err(RuntimeGatewayReachabilityError::ProtocolViolation);
            }
            let runtime = entry
                .runtime
                .clone()
                .ok_or(RuntimeGatewayReachabilityError::RuntimeUnavailable)?;
            (
                runtime,
                entry.session.kernel.clone(),
                entry.subject_credential.is_some(),
            )
        };
        if let Some(expected) = expected {
            if !credential_acknowledged || expected.session_id != *id {
                return Err(RuntimeGatewayReachabilityError::CredentialAuthorityDrift);
            }
            let source = self
                .credentials
                .as_ref()
                .ok_or(RuntimeGatewayReachabilityError::CredentialAuthorityDrift)?;
            let current = source
                .credential_authority(id)
                .await
                .map_err(|_| RuntimeGatewayReachabilityError::TransientBrokerFailure)?;
            if current.as_ref() != Some(expected) {
                return Err(RuntimeGatewayReachabilityError::CredentialAuthorityDrift);
            }
        }
        let adapter = self
            .adapters
            .get(&kernel)
            .ok_or(RuntimeGatewayReachabilityError::ProtocolUnsupported)?;
        let envelope = build(&runtime.0).map_err(RuntimeGatewayReachabilityError::Core)?;
        let policy = adapter
            .install_runtime_policy(&runtime, &envelope)
            .await
            .map_err(map_policy_adapter_error)?;
        let probe = adapter
            .probe_runtime_gateway(&runtime, &request)
            .await
            .map_err(map_probe_adapter_error)?;
        if probe.version != request.version {
            return Err(RuntimeGatewayReachabilityError::ProtocolViolation);
        }
        match probe.outcome {
            layer3_adapter::RuntimeGatewayProbeOutcome::Allowed => {
                Ok(RuntimeGatewayReachabilityReceipt {
                    runtime_audience: runtime.0,
                    policy,
                    probe_version: probe.version,
                })
            }
            layer3_adapter::RuntimeGatewayProbeOutcome::CommandSandboxInitializationFailed => {
                Err(RuntimeGatewayReachabilityError::CommandSandboxInitialization)
            }
            layer3_adapter::RuntimeGatewayProbeOutcome::GatewayNetworkInitializationFailed => {
                Err(RuntimeGatewayReachabilityError::GatewayNetworkInitialization)
            }
            layer3_adapter::RuntimeGatewayProbeOutcome::PolicyDenied => {
                Err(RuntimeGatewayReachabilityError::PolicyDenied)
            }
            layer3_adapter::RuntimeGatewayProbeOutcome::ProviderDenied => {
                Err(RuntimeGatewayReachabilityError::ProviderDenied)
            }
            layer3_adapter::RuntimeGatewayProbeOutcome::TransientBrokerFailure => {
                Err(RuntimeGatewayReachabilityError::TransientBrokerFailure)
            }
        }
    }

    /// Permanently retire the credential assigned to one Trigger session.
    /// This leaves the runtime and conversation alive.
    pub async fn retire_runtime_credential(&self, id: &SessionId) -> Result<(), CoreError> {
        let source = self.credentials.as_ref().ok_or_else(|| {
            CoreError::RuntimeUnavailable("session credential custody is unavailable".into())
        })?;
        source
            .release(id)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }

    /// Positive proof that credential custody settled provider revocation.
    pub async fn runtime_credential_retirement_settled(
        &self,
        id: &SessionId,
    ) -> Result<bool, CoreError> {
        let source = self.credentials.as_ref().ok_or_else(|| {
            CoreError::RuntimeUnavailable("session credential custody is unavailable".into())
        })?;
        source
            .release_settled(id)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }
}

fn map_policy_adapter_error(
    error: layer3_adapter::AdapterError,
) -> RuntimeGatewayReachabilityError {
    match error {
        layer3_adapter::AdapterError::Unavailable(_) => {
            RuntimeGatewayReachabilityError::RuntimeUnavailable
        }
        layer3_adapter::AdapterError::Unsupported(_) => {
            RuntimeGatewayReachabilityError::ProtocolUnsupported
        }
        layer3_adapter::AdapterError::Rejected(_) | layer3_adapter::AdapterError::AtCapacity(_) => {
            RuntimeGatewayReachabilityError::ProtocolViolation
        }
    }
}

fn map_probe_adapter_error(error: layer3_adapter::AdapterError) -> RuntimeGatewayReachabilityError {
    map_policy_adapter_error(error)
}

fn map_policy_observation_adapter_error(error: layer3_adapter::AdapterError) -> CoreError {
    match error {
        layer3_adapter::AdapterError::Unavailable(_) => {
            CoreError::RuntimeUnavailable("runtime policy observation was unavailable".into())
        }
        layer3_adapter::AdapterError::Unsupported(_) => {
            CoreError::CapabilityUnsupported("authenticated_runtime_policy_observation".into())
        }
        layer3_adapter::AdapterError::Rejected(_) | layer3_adapter::AdapterError::AtCapacity(_) => {
            CoreError::RuntimeRefused("runtime policy observation was malformed".into())
        }
    }
}

fn map_runtime_policy_refusal(value: layer3_adapter::RuntimePolicyRefusal) -> CapabilityRefusal {
    use layer3_adapter::RuntimePolicyRefusal as Runtime;
    match value {
        Runtime::MissingPolicy => CapabilityRefusal::MissingPolicy,
        Runtime::UnknownVersion => CapabilityRefusal::UnknownVersion,
        Runtime::Expired => CapabilityRefusal::Expired,
        Runtime::StaleGeneration => CapabilityRefusal::StaleGeneration,
        Runtime::Revoked => CapabilityRefusal::Revoked,
        Runtime::MalformedRequest => CapabilityRefusal::MalformedRequest,
        Runtime::ResourceMismatch => CapabilityRefusal::ResourceMismatch,
        Runtime::OperationDenied => CapabilityRefusal::OperationDenied,
        Runtime::AudienceMismatch => CapabilityRefusal::AudienceMismatch,
        Runtime::SignatureInvalid => CapabilityRefusal::SignatureInvalid,
    }
}
