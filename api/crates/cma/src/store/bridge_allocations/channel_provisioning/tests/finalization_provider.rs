//! Test provider: native binding identities are captured on dispatch and
//! persisted by production journals. No standalone database proof is seeded.
use layer3_adapter::*;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct ProviderFixture {
    original: tokio::sync::Mutex<BTreeMap<String, RuntimeProvisionReceipt>>,
    removed: tokio::sync::Mutex<BTreeMap<String, RuntimeFinalityReceipt>>,
}
impl ProviderFixture {
    pub async fn provision(
        &self,
        adapter: &dyn SandboxAdapter,
        request: &RuntimeProvisionRequest,
        operation: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        let mut originals = self.original.lock().await;
        if let Some(prior) = originals.get(operation) {
            return Ok(prior.clone());
        }
        let handle = adapter.provision_runtime(request).await?;
        let scope = HomeVolumeScope::from_authority("fixture-native", &["history-finalization"])?;
        let receipt: RuntimeProvisionReceipt = serde_json::from_value(json!({
            "operation_id": operation, "request_sha256": provider_request_digest(request)?,
            "provider_response_sha256": provider_request_digest(&handle)?, "pre_dispatch_list_version": "9",
            "handle": handle, "incarnation": {"scope": scope, "namespace": "fixture",
                "name": handle.0, "uid": format!("native-{}", uuid::Uuid::new_v4()),
                "generation": 1, "binding_resource_version": "10", "identity_sha256": provider_request_digest(request)?},
        }))
        .unwrap();
        receipt.validate()?;
        originals.insert(operation.into(), receipt.clone());
        Ok(receipt)
    }
    pub async fn finalize(
        &self,
        adapter: &dyn SandboxAdapter,
        original: &RuntimeProvisionReceipt,
        operation: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        if self.original.lock().await.get(original.operation_id()) != Some(original) {
            return Err(AdapterError::Rejected(
                "original native identity mismatch".into(),
            ));
        }
        let mut removed = self.removed.lock().await;
        if let Some(prior) = removed.get(operation) {
            return Ok(prior.clone());
        }
        adapter.teardown(original.handle()).await?;
        assert!(!adapter
            .runtime_inventory()
            .await?
            .iter()
            .any(|row| &row.handle == original.handle()));
        let receipt: RuntimeFinalityReceipt = serde_json::from_value(json!({
            "original": original, "delete_operation_id": operation,
            "delete_resource_version": "fixture-delete-v1", "delete_response_sha256": provider_request_digest(original)?,
            "absent_runtime_list_version": "fixture-runtime-list-v2", "absent_owned_pods_list_version": "fixture-pod-list-v2",
        })).unwrap();
        receipt.validate()?;
        removed.insert(operation.into(), receipt.clone());
        Ok(receipt)
    }
}
