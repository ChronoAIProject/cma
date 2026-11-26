//! Existing binding exchange negotiates optional diagnostics without a retry.
use super::client::{acquire, transport_error};
use super::{BindingRequest, BindingResponse, ProviderClient, ProviderError};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ProviderSlug},
    profile::canonical_json,
};
use serde_json::json;

impl ProviderClient {
    pub(crate) fn has_cma_binding_contract(&self, provider: &ProviderSlug) -> bool {
        provider.as_str() == "cma_codex"
            && self.handle(provider).is_ok_and(|handle| {
                handle.entry.paths.binding == "/api/v1/cmaeg/bindings/{binding_id}"
            })
    }

    pub async fn bind(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        request: &BindingRequest,
    ) -> Result<BindingResponse, ProviderError> {
        self.bind_custodied(provider, binding_id, request)
            .await
            .map_err(|error| error.error)
    }
    pub(crate) async fn bind_custodied(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        request: &BindingRequest,
    ) -> Result<BindingResponse, MutationError> {
        self.record_worker_key(&request.agent_key).await?;
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url("binding", &[("binding_id", binding_id.as_str())])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let mut body = json!({
            "schema_version": request.schema_version,
            "profile": request.profile,
            "profile_digest": request.profile_digest,
            "agent_key": request.agent_key.as_str(),
            "tools": request.tools,
        });
        if !request.target.is_profile() {
            body["target"] = serde_json::to_value(&request.target)
                .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        }
        let requested_trace = self.turn_trace_enabled && provider.as_str() == "nyxbot";
        if requested_trace {
            body["capabilities"] = json!([super::trace_contract::CAPABILITY]);
        }
        let bearer = self
            .management_bearer(provider, &url, &request.agent_key)
            .await?;
        let response = handle
            .client
            .put(url)
            .bearer_auth(bearer.as_str())
            // Keep the reviewed provider bytes independent of serde_json's
            // workspace-unified preserve_order feature, including nested values.
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(canonical_json(body))
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| MutationError::unknown(transport_error(error)))?;
        let response = self
            .success_or_error(response)
            .await
            .map_err(MutationError::unknown)?;
        let mut binding = response.json::<BindingResponse>().await.map_err(|_| {
            MutationError::unknown(ProviderError::ResponseMalformed(
                "invalid binding response".into(),
            ))
        })?;
        if binding.binding_id != *binding_id {
            return Err(MutationError::unknown(ProviderError::ResponseMalformed(
                "binding response identity mismatch".into(),
            )));
        }
        if !requested_trace {
            binding.capabilities.clear();
        }
        Ok(binding)
    }
}

impl ProviderClient {
    pub async fn unbind(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        agent_key: &Secret,
    ) -> Result<(), ProviderError> {
        self.unbind_custodied(provider, binding_id, agent_key)
            .await
            .map_err(|error| error.error)
    }
    pub(crate) async fn unbind_custodied(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        agent_key: &Secret,
    ) -> Result<(), MutationError> {
        self.record_worker_key(agent_key).await?;
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url("binding", &[("binding_id", binding_id.as_str())])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let bearer = self.management_bearer(provider, &url, agent_key).await?;
        let response = handle
            .client
            .delete(url)
            .bearer_auth(bearer.as_str())
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| MutationError::unknown(transport_error(error)))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        self.success_or_error(response)
            .await
            .map(|_| ())
            .map_err(MutationError::unknown)
    }
}

#[cfg(test)]
mod tests;

/// The dispatch boundary retains proof of a local refusal without guessing from
/// broad transport/status failures after a provider request has been issued.
pub(crate) struct MutationError {
    pub(crate) error: ProviderError,
    pub(crate) unconfirmed: bool,
}
impl From<ProviderError> for MutationError {
    fn from(error: ProviderError) -> Self {
        Self {
            error,
            unconfirmed: false,
        }
    }
}
impl MutationError {
    fn unknown(error: ProviderError) -> Self {
        Self {
            error,
            unconfirmed: true,
        }
    }
}
