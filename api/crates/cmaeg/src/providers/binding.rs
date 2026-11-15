//! Existing binding exchange negotiates optional diagnostics without a retry.
use super::client::{acquire, transport_error};
use super::{BindingRequest, BindingResponse, ProviderClient, ProviderError};
use cmaeg_core::{
    ids::{BindingId, ProviderSlug},
    profile::canonical_json,
};
use serde_json::json;

impl ProviderClient {
    pub async fn bind(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        request: &BindingRequest,
    ) -> Result<BindingResponse, ProviderError> {
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
            .map_err(transport_error)?;
        let response = self.success_or_error(response).await?;
        let mut binding = response
            .json::<BindingResponse>()
            .await
            .map_err(|_| ProviderError::ResponseMalformed("invalid binding response".into()))?;
        if binding.binding_id != *binding_id {
            return Err(ProviderError::ResponseMalformed(
                "binding response identity mismatch".into(),
            ));
        }
        if !requested_trace {
            binding.capabilities.clear();
        }
        Ok(binding)
    }
}

#[cfg(test)]
mod tests;
