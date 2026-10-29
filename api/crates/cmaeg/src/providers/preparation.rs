//! Explicit management-authenticated preparation. No retry loop: a caller must
//! durably retain the exact intent before invoking the reservation endpoint.
use super::{
    client::{acquire, transport_error},
    ProviderClient, ProviderError,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    turns::routed::{BindingPreparation, PreparationIntent, PreparationReceipt},
};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ProviderSlug},
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

impl ProviderClient {
    pub fn supports_binding_preparation(&self, provider: &ProviderSlug) -> bool {
        let Ok(handle) = self.handle(provider) else {
            return false;
        };
        ["preparation_inspection", "preparation_reservation"]
            .into_iter()
            .all(|path| {
                handle
                    .entry
                    .url(path, &[("binding_id", "binding_capability_probe")])
                    .is_ok_and(|url| self.permits_preparation(provider, &url))
            })
    }

    pub async fn inspect_binding_preparation(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        credential: &Secret,
    ) -> Result<BindingPreparation, ProviderError> {
        self.record_worker_key(credential).await?;
        if !plan.prepares_binding(target) {
            return Err(ProviderError::InvalidRequest(
                "admission is not eligible for binding preparation".into(),
            ));
        }
        let result: BindingPreparation = self.preparation_request(
            &target.launch.provider.slug, &target.launch.provider.binding_id,
            "preparation_inspection", None,
            json!({"schema_version":1, "admission_id":plan.admission_id, "target_id":target.id,
                "request_hash":plan.request_hash, "owner":plan.identity.owner, "admitted_at":plan.admitted_at,
                "expires_at":target.launch.event.expires_at.min(plan.retained_until), "agent_key":credential.as_str()}),
        ).await?;
        if matches!(
            result,
            BindingPreparation::Reserved { .. } | BindingPreparation::Inherited { .. }
        ) {
            return Err(ProviderError::ResponseMalformed(
                "inspection returned a reservation".into(),
            ));
        }
        Ok(result)
    }

    pub async fn reserve_binding_preparation(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        intent: &PreparationIntent,
        credential: &Secret,
    ) -> Result<PreparationReceipt, ProviderError> {
        self.record_worker_key(credential).await?;
        if intent.binding_id != binding.as_str() {
            return Err(ProviderError::InvalidRequest(
                "binding preparation identity differs".into(),
            ));
        }
        let operation = intent
            .operation_id()
            .map_err(|_| ProviderError::InvalidRequest("invalid binding preparation".into()))?;
        self.preparation_request(
            provider,
            binding,
            "preparation_reservation",
            Some(&operation),
            json!({"schema_version":1,"intent":intent,"agent_key":credential.as_str()}),
        )
        .await
    }

    async fn preparation_request<T: DeserializeOwned>(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        path: &str,
        operation: Option<&str>,
        body: Value,
    ) -> Result<T, ProviderError> {
        let handle = self.handle(provider)?;
        let url = handle
            .entry
            .url(path, &[("binding_id", binding.as_str())])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let bearer = self.preparation_bearer(provider, &url).await?;
        let _permit = acquire(&handle.inflight).await?;
        let mut request = handle
            .client
            .post(url)
            .bearer_auth(bearer.as_str())
            .timeout(self.request_timeout)
            .json(&body);
        if let Some(operation) = operation {
            request = request.header("idempotency-key", operation);
        }
        let response = request.send().await.map_err(transport_error)?;
        let mut response = self.success_or_error(response).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if bytes.len().saturating_add(chunk.len()) > 32768 {
                return Err(ProviderError::ResponseMalformed(
                    "binding preparation response is too large".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            ProviderError::ResponseMalformed("invalid binding preparation response".into())
        })
    }
}

#[cfg(test)]
#[path = "preparation_tests.rs"]
mod tests;
