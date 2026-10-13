//! Retain one CRD and PVC while replacing only the stopped Pod incarnation.
use super::*;
use crate::provider::ProviderSandboxRequest;
use kube::api::{Patch, PatchParams};
use serde_json::json;
use std::time::Duration;

impl AgentSandboxClient {
    /// Idempotent suspension requires the exact admitted spec, UID and Pod absence.
    pub async fn suspend(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        let (name, _) = parse_handle(runtime)?;
        let mut desired = self.desired_at(request, name).await?;
        let actual = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Sandbox is absent"))?;
        let mode = actual
            .data
            .pointer("/spec/operatingMode")
            .and_then(|v| v.as_str());
        if !matches!(mode, Some("Running" | "Suspended")) {
            return Err(rejected("Sandbox operating mode is unsupported"));
        }
        desired.data["spec"]["operatingMode"] = json!(mode);
        self.matches(&actual, &desired)?;
        if mode != Some("Suspended") {
            let version = actual
                .resource_version()
                .ok_or_else(|| rejected("missing resource version"))?;
            let patch = json!({"metadata": {"uid": actual.uid(),
                "resourceVersion": version},
                "spec": {"operatingMode": "Suspended"}});
            self.sandboxes
                .patch(name, &PatchParams::default(), &Patch::Merge(patch))
                .await
                .map_err(api_error)?;
        }
        desired.data["spec"]["operatingMode"] = json!("Suspended");
        self.wait_suspended(runtime, &desired).await
    }

    pub async fn resume(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<()> {
        validate_successor(previous, next)?;
        let (name, _) = parse_handle(runtime)?;
        let mut before = self.desired_at(previous, name).await?;
        before.data["spec"]["operatingMode"] = json!("Suspended");
        let desired = self.desired_at(next, name).await?;
        self.verify_explicit_runtime_policy(&desired).await?;
        let actual = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Suspended Sandbox is absent"))?;
        // A lost acknowledgement may leave the admitted next generation Running.
        // This does not authorize another generation or another creation.
        if self.matches(&actual, &desired).is_ok() {
            return Ok(());
        }
        self.matches(&actual, &before)?;
        self.wait_suspended(runtime, &before).await?;
        let actual = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Suspended Sandbox is absent"))?;
        self.matches(&actual, &before)?;
        let version = actual
            .resource_version()
            .ok_or_else(|| rejected("missing resource version"))?;
        let patch = json!({
            "metadata": {"uid": actual.uid(), "resourceVersion": version,
                "annotations": desired.annotations()},
            "spec": desired.data["spec"],
        });
        let updated = self
            .sandboxes
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(api_error)?;
        if updated.uid() != actual.uid() {
            return Err(rejected("Sandbox UID changed during resume"));
        }
        self.matches(&updated, &desired)
    }

    /// Observation is also used after restart, without requiring a live runtime HTTP endpoint.
    pub async fn verify_suspended(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        let (name, _) = parse_handle(runtime)?;
        let mut desired = self.desired_at(request, name).await?;
        desired.data["spec"]["operatingMode"] = json!("Suspended");
        self.wait_suspended(runtime, &desired).await
    }

    async fn wait_suspended(&self, runtime: &RuntimeHandle, desired: &DynamicObject) -> Result<()> {
        let (name, uid) = parse_handle(runtime)?;
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let actual = self
                    .get(runtime)
                    .await?
                    .ok_or_else(|| unavailable("Suspended Sandbox is absent"))?;
                self.matches(&actual, desired)?;
                let pods = self
                    .pods
                    .list(
                        &ListParams::default()
                            .labels(&pods::selector(name))
                            .limit(100),
                    )
                    .await
                    .map_err(api_error)?;
                if pods
                    .metadata
                    .continue_
                    .is_some_and(|cursor| !cursor.is_empty())
                {
                    return Err(unavailable(
                        "Sandbox suspension Pod inventory is incomplete",
                    ));
                }
                if !pods.items.iter().any(|pod| {
                    pod.metadata
                        .owner_references
                        .as_ref()
                        .is_some_and(|owners| owners.iter().any(|owner| owner.uid == uid))
                }) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .map_err(|_| unavailable("Sandbox suspension is waiting for Pod removal"))?
    }
}

fn validate_successor(
    previous: &ProviderSandboxRequest,
    next: &ProviderSandboxRequest,
) -> Result<()> {
    let generation = |request: &ProviderSandboxRequest| {
        request
            .metadata
            .get("cma/runtime-generation")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
    };
    let (Some(old), Some(new)) = (generation(previous), generation(next)) else {
        return Err(rejected("Sandbox resume generation is absent"));
    };
    let mut expected = previous.clone();
    expected
        .metadata
        .insert("cma/runtime-generation".into(), new.to_string());
    expected
        .env
        .insert("CHRONO_RUNTIME_GENERATION".into(), new.to_string());
    // The rendered fingerprint includes generation. All other request fields
    // must remain exact, including image, home UID, mounts and service scope.
    expected.metadata.remove("cma/spec-sha256");
    let mut submitted = next.clone();
    submitted.metadata.remove("cma/spec-sha256");
    if new <= old || previous.volumes.is_empty() || expected != submitted {
        return Err(rejected(
            "Sandbox resume must advance only its runtime generation",
        ));
    }
    Ok(())
}
