//! Installing one sponsored-model capability on one exact live runtime.

use crate::{
    BillingMode, InertGrantCapability, SessionCredentialAuthority, SessionId, SessionService,
};

/// Closed failure vocabulary for the pre-prompt sponsored-model authority gate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsoredModelAuthorityError {
    #[error("the sponsored model authority is not valid for this session")]
    ScopeMismatch,
    #[error("the live runtime or its control link is unavailable")]
    RuntimeUnavailable,
    #[error("the runtime does not support sponsored model authority")]
    ProtocolUnsupported,
    #[error("the runtime refused or malformed the sponsored model authority")]
    ProtocolViolation,
    #[error("the independent session credential authority changed")]
    CredentialAuthorityDrift,
    #[error("session credential custody is temporarily unavailable")]
    CredentialAuthorityUnavailable,
}

impl SessionService {
    /// Verify that one request came from the current live runtime for the
    /// sponsored Trigger Launch named by its inert capability.
    ///
    /// The caller still has to authenticate the independently delivered
    /// session credential. This check carries no bearer material and cannot
    /// authorize spend by itself.
    pub async fn verifies_current_sponsored_model_runtime(
        &self,
        id: &SessionId,
        launch_id: &crate::LaunchId,
        runtime_audience: &str,
    ) -> bool {
        if runtime_audience.is_empty() || runtime_audience.len() > 2_048 {
            return false;
        }
        let _lifecycle = self.lock_lifecycle(id).await;
        let entries = self.entries.read().await;
        let Some(entry) = entries.get(id) else {
            return false;
        };
        entry
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.0 == runtime_audience)
            && entry.trigger_origin.as_ref().is_some_and(|origin| {
                origin.billing_mode == BillingMode::Sponsored && origin.launch_id == *launch_id
            })
    }

    /// Install one exact inert grant on the current sponsored runtime.
    ///
    /// The lifecycle guard spans runtime selection, independent credential
    /// verification, installation, and receipt validation. A successful return
    /// therefore cannot combine the credential from one runtime generation with
    /// an acknowledgement from another. Provider grant or credential material
    /// is absent from this contract.
    pub async fn install_sponsored_model_authority(
        &self,
        id: &SessionId,
        expected_credential: &SessionCredentialAuthority,
        capability: &InertGrantCapability,
        maximum_output_tokens_per_request: u64,
        gateway_origin: &layer3_adapter::SponsoredModelGatewayOrigin,
    ) -> Result<layer3_adapter::SponsoredModelAuthorityReceipt, SponsoredModelAuthorityError> {
        capability
            .validate()
            .map_err(|_| SponsoredModelAuthorityError::ScopeMismatch)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if crate::seconds_from_rfc3339(&capability.expires_at)
            .is_none_or(|expires_at| expires_at <= now)
        {
            return Err(SponsoredModelAuthorityError::ScopeMismatch);
        }
        if capability.session_id != *id
            || expected_credential.session_id != *id
            || maximum_output_tokens_per_request == 0
            || maximum_output_tokens_per_request > u64::from(u32::MAX)
        {
            return Err(SponsoredModelAuthorityError::ScopeMismatch);
        }

        let _lifecycle = self.lock_lifecycle(id).await;
        let (runtime, kernel, origin, credential_acknowledged) = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(id)
                .ok_or(SponsoredModelAuthorityError::RuntimeUnavailable)?;
            let origin = entry
                .trigger_origin
                .clone()
                .ok_or(SponsoredModelAuthorityError::ScopeMismatch)?;
            let runtime = entry
                .runtime
                .clone()
                .ok_or(SponsoredModelAuthorityError::RuntimeUnavailable)?;
            (
                runtime,
                entry.session.kernel.clone(),
                origin,
                entry.subject_credential.is_some(),
            )
        };

        if origin.billing_mode != BillingMode::Sponsored
            || origin.launch_id != capability.launch_id
            || !credential_acknowledged
        {
            return Err(SponsoredModelAuthorityError::ScopeMismatch);
        }
        let credential_source = self
            .credentials
            .as_ref()
            .ok_or(SponsoredModelAuthorityError::CredentialAuthorityDrift)?;
        let current = credential_source
            .credential_authority(id)
            .await
            .map_err(|_| SponsoredModelAuthorityError::CredentialAuthorityUnavailable)?;
        if current.as_ref() != Some(expected_credential) {
            return Err(SponsoredModelAuthorityError::CredentialAuthorityDrift);
        }

        let authority = layer3_adapter::SponsoredModelRuntimeAuthority {
            version: layer3_adapter::SPONSORED_MODEL_RUNTIME_AUTHORITY_VERSION,
            gateway_origin: gateway_origin.clone(),
            runtime_audience: runtime.0.clone(),
            session_id: id.0.clone(),
            session_credential_generation: expected_credential.custody_generation,
            launch_id: capability.launch_id.0.clone(),
            grant_id: capability.grant_id.0.clone(),
            grant_generation: capability.grant_generation,
            policy_generation: capability.policy_generation,
            maximum_output_tokens_per_request,
            expires_at: capability.expires_at.clone(),
            capability_fingerprint: capability.fingerprint.clone(),
        };
        let adapter = self
            .adapters
            .get(&kernel)
            .ok_or(SponsoredModelAuthorityError::ProtocolUnsupported)?;
        let receipt = adapter
            .install_sponsored_model_authority(&runtime, &authority)
            .await
            .map_err(map_adapter_error)?;
        let expected = layer3_adapter::SponsoredModelAuthorityReceipt {
            runtime_audience: authority.runtime_audience,
            session_id: authority.session_id,
            session_credential_generation: authority.session_credential_generation,
            launch_id: authority.launch_id,
            grant_id: authority.grant_id,
            grant_generation: authority.grant_generation,
            policy_generation: authority.policy_generation,
            maximum_output_tokens_per_request: authority.maximum_output_tokens_per_request,
            capability_fingerprint: authority.capability_fingerprint,
        };
        if receipt != expected {
            return Err(SponsoredModelAuthorityError::ProtocolViolation);
        }
        Ok(receipt)
    }
}

fn map_adapter_error(error: layer3_adapter::AdapterError) -> SponsoredModelAuthorityError {
    match error {
        layer3_adapter::AdapterError::Unavailable(_) => {
            SponsoredModelAuthorityError::RuntimeUnavailable
        }
        layer3_adapter::AdapterError::Unsupported(_) => {
            SponsoredModelAuthorityError::ProtocolUnsupported
        }
        layer3_adapter::AdapterError::Rejected(_) | layer3_adapter::AdapterError::AtCapacity(_) => {
            SponsoredModelAuthorityError::ProtocolViolation
        }
    }
}
