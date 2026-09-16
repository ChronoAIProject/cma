use super::*;
use crate::{
    AdapterError, RuntimeFinalityReceipt, RuntimeProviderIncarnation, RuntimeProvisionReceipt,
};

pub(crate) const DISPATCH_LABEL: &str = "cma/provider-dispatch";

pub(crate) struct PreDispatchObservation {
    resource_version: String,
    marker: String,
}

#[cfg(test)]
#[path = "runtime_finality_tests.rs"]
mod tests;

pub(crate) fn dispatch_marker(operation_id: &str) -> crate::Result<String> {
    if !crate::provider_finality::identifier(operation_id) {
        return Err(AdapterError::Rejected(
            "provider dispatch operation is invalid".into(),
        ));
    }
    Ok(format!(
        "d{}",
        &crate::provider_request_digest(&operation_id)?[..60]
    ))
}

impl BatchSandboxReclaimer {
    pub(crate) async fn observe_before_dispatch(
        &self,
        operation_id: &str,
    ) -> crate::Result<PreDispatchObservation> {
        self.require_finality_scope()?;
        let marker = dispatch_marker(operation_id)?;
        let rows = self
            .api
            .list(
                &ListParams::default()
                    .labels(&format!("{DISPATCH_LABEL}={marker}"))
                    .limit(2),
            )
            .await
            .map_err(|error| kube_error("observe runtime dispatch marker before POST", error))?;
        if !rows.items.is_empty()
            || rows
                .metadata
                .continue_
                .as_ref()
                .is_some_and(|v| !v.is_empty())
        {
            return Err(AdapterError::Rejected(
                "provider dispatch marker already exists or its inventory is incomplete".into(),
            ));
        }
        let resource_version = rows
            .metadata
            .resource_version
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                AdapterError::Unavailable("pre-dispatch observation has no version".into())
            })?;
        Ok(PreDispatchObservation {
            resource_version,
            marker,
        })
    }
    pub(crate) fn require_finality_scope(&self) -> crate::Result<()> {
        self.scope
            .as_ref()
            .ok_or_else(|| {
                AdapterError::Unsupported("runtime provider scope is unavailable".into())
            })?
            .validate()
    }
    pub fn with_finality_authority(mut self, api_authority: &str) -> Result<Self, ProviderError> {
        self.scope = Some(
            crate::HomeVolumeScope::from_authority(
                "kubernetes-runtime-v1",
                &[api_authority, &self.namespace],
            )
            .map_err(|_| ProviderError::Malformed("runtime provider scope is invalid".into()))?,
        );
        Ok(self)
    }

    fn incarnation(&self, captured: &CapturedSandbox) -> crate::Result<RuntimeProviderIncarnation> {
        Ok(RuntimeProviderIncarnation {
            scope: self.scope.clone().ok_or_else(|| {
                AdapterError::Unsupported("runtime provider scope is unavailable".into())
            })?,
            namespace: self.namespace.clone(),
            name: captured.handle.0.clone(),
            uid: captured.identity.uid.clone(),
            generation: captured.identity.generation,
            binding_resource_version: captured.resource_version.clone(),
            identity_sha256: crate::provider_request_digest(&captured.identity)?,
        })
    }

    pub(crate) async fn capture_for_binding(
        &self,
        handle: &RuntimeHandle,
        operation_id: &str,
        request_sha256: &str,
        provider_response_sha256: &str,
        expected_metadata: &BTreeMap<String, String>,
        observation: PreDispatchObservation,
    ) -> crate::Result<RuntimeProvisionReceipt> {
        if observation.marker != dispatch_marker(operation_id)?
            || expected_metadata.get(DISPATCH_LABEL) != Some(&observation.marker)
        {
            return Err(AdapterError::Rejected(
                "pre-dispatch observation belongs to another request".into(),
            ));
        }
        // The successful POST authorizes the logical handle. This namespaced
        // GET pins the incarnation CMA will adopt before binding or installation;
        // it makes no claim about the first-ever object behind that name.
        let captured = self
            .capture(handle)
            .await?
            .ok_or_else(|| AdapterError::Unavailable("runtime is absent before binding".into()))?;
        let mut actual_metadata = captured.identity.labels.clone();
        crate::provider::metadata::decode(&mut actual_metadata);
        if expected_metadata
            .iter()
            .any(|(key, value)| actual_metadata.get(key) != Some(value))
        {
            return Err(AdapterError::Rejected(
                "runtime metadata does not match the dispatched request".into(),
            ));
        }
        let receipt = RuntimeProvisionReceipt {
            operation_id: operation_id.to_owned(),
            request_sha256: request_sha256.to_owned(),
            provider_response_sha256: provider_response_sha256.to_owned(),
            handle: handle.clone(),
            incarnation: self.incarnation(&captured)?,
            pre_dispatch_list_version: observation.resource_version,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub(crate) async fn require_bound_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
    ) -> crate::Result<()> {
        original.validate()?;
        let current = self
            .capture(original.handle())
            .await?
            .ok_or_else(|| AdapterError::Unavailable("bound runtime no longer exists".into()))?;
        let mut identity = self.incarnation(&current)?;
        identity.binding_resource_version = original.incarnation.binding_resource_version.clone();
        if identity != original.incarnation {
            return Err(AdapterError::Rejected(
                "bound provider runtime incarnation changed".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn finalize_bound_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<RuntimeFinalityReceipt> {
        original.validate()?;
        if !crate::provider_finality::identifier(operation_id) {
            return Err(AdapterError::Rejected("delete operation is invalid".into()));
        }
        let captured = self.capture(original.handle()).await?.ok_or_else(|| {
            AdapterError::Unavailable(
                "no bound provider runtime is available for receipted deletion".into(),
            )
        })?;
        let mut identity = self.incarnation(&captured)?;
        identity.binding_resource_version = original.incarnation.binding_resource_version.clone();
        if identity != original.incarnation {
            return Err(AdapterError::Rejected(
                "provider finality target was replaced".into(),
            ));
        }
        let params = DeleteParams {
            grace_period_seconds: Some(0),
            propagation_policy: Some(kube::api::PropagationPolicy::Foreground),
            preconditions: Some(Preconditions {
                uid: Some(captured.identity.uid.clone()),
                resource_version: Some(captured.resource_version.clone()),
            }),
            ..DeleteParams::default()
        };
        // Missing/lost responses never become receipts from later observations.
        let response = self
            .api
            .delete(original.handle().as_str(), &params)
            .await
            .map_err(|error| kube_error("delete bound provider runtime", error))?;
        let response_sha256 = response.either(
            |value| crate::provider_request_digest(&value),
            |value| crate::provider_request_digest(&value),
        )?;
        let (runtime_version, pod_version) = tokio::time::timeout(DELETE_CONFIRM_TIMEOUT, async {
            loop {
                let rows = self
                    .api
                    .list(
                        &ListParams::default()
                            .fields(&format!("metadata.name={}", original.handle().as_str())),
                    )
                    .await
                    .map_err(|error| kube_error("observe bound runtime absence", error))?;
                if rows
                    .metadata
                    .continue_
                    .as_ref()
                    .is_some_and(|v| !v.is_empty())
                    || rows.items.len() > 1
                {
                    return Err(AdapterError::Rejected(
                        "runtime absence listing is incomplete".into(),
                    ));
                }
                if rows.items.iter().any(|row| {
                    row.metadata.uid.as_deref() == Some(original.incarnation.uid.as_str())
                }) {
                    tokio::time::sleep(DELETE_CONFIRM_POLL).await;
                    continue;
                }
                let runtime_version = rows
                    .metadata
                    .resource_version
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| {
                        AdapterError::Unavailable(
                            "runtime absence has no observation version".into(),
                        )
                    })?;
                if let Some(pod_version) = self.absent_owned_pods(&original.incarnation.uid).await?
                {
                    return Ok((runtime_version, pod_version));
                }
                tokio::time::sleep(DELETE_CONFIRM_POLL).await;
            }
        })
        .await
        .map_err(|_| {
            AdapterError::Unavailable("bound provider runtime finality is still pending".into())
        })??;
        let receipt = RuntimeFinalityReceipt {
            original: original.clone(),
            delete_operation_id: operation_id.to_owned(),
            delete_resource_version: captured.resource_version,
            delete_response_sha256: response_sha256,
            absent_runtime_list_version: runtime_version,
            absent_owned_pods_list_version: pod_version,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    async fn absent_owned_pods(&self, owner_uid: &str) -> crate::Result<Option<String>> {
        let mut params = ListParams::default().limit(200);
        let mut cursors = std::collections::BTreeSet::new();
        let mut version = None;
        let mut count = 0;
        loop {
            let page = self
                .pods
                .list(&params)
                .await
                .map_err(|error| kube_error("observe bound runtime owned pods", error))?;
            let current = page
                .metadata
                .resource_version
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    AdapterError::Unavailable("pod absence has no observation version".into())
                })?;
            if version.as_ref().is_some_and(|old| old != &current) {
                return Err(AdapterError::Unavailable(
                    "pod inventory changed during observation".into(),
                ));
            }
            version = Some(current);
            count += page.items.len();
            if count > 10_000 {
                return Err(AdapterError::Unavailable(
                    "pod inventory exceeded finality bound".into(),
                ));
            }
            if page.items.iter().any(|pod| {
                pod.metadata
                    .owner_references
                    .as_ref()
                    .is_some_and(|owners| owners.iter().any(|owner| owner.uid == owner_uid))
            }) {
                return Ok(None);
            }
            let Some(cursor) = page.metadata.continue_.filter(|v| !v.is_empty()) else {
                return Ok(version);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(AdapterError::Unavailable(
                    "pod inventory did not advance".into(),
                ));
            }
            params = params.continue_token(&cursor);
        }
    }
}
