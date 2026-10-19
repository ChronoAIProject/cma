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
        self.resume_inner(runtime, previous, next, None).await
    }

    pub(crate) async fn resume_with_home(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        self.resume_inner(
            runtime,
            previous,
            next,
            home.request.policy.is_native_vct().then_some(home),
        )
        .await
    }

    async fn resume_inner(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: Option<&crate::HomeVolumeAttachment>,
    ) -> Result<()> {
        let mut timing = resume_timing::ResumeTiming::new();
        validate_successor(previous, next)?;
        let (name, _) = parse_handle(runtime)?;
        timing.completed("local_guard");
        let (mut before, desired) = if let Some(home) = home {
            self.native_resume_desired(runtime, previous, next, home)
                .await?
        } else {
            (
                self.desired_at(previous, name).await?,
                self.desired_at(next, name).await?,
            )
        };
        before.data["spec"]["operatingMode"] = json!("Suspended");
        self.verify_explicit_runtime_policy(&desired).await?;
        timing.completed("generation_proof_render");
        let actual = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Suspended Sandbox is absent"))?;
        // A lost acknowledgement may leave the admitted next generation Running.
        // This does not authorize another generation or another creation.
        if self.matches(&actual, &desired).is_ok() {
            timing.completed("already_resumed");
            return Ok(());
        }
        self.matches(&actual, &before)?;
        self.wait_suspended(runtime, &before).await?;
        timing.completed("wait_suspended");
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
        self.matches(&updated, &desired)?;
        timing.completed("generation_patch");
        Ok(())
    }

    async fn native_resume_desired(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<(DynamicObject, DynamicObject)> {
        let (name, _) = parse_handle(runtime)?;
        // Keep the generic request guard before resource I/O, as desired_at does.
        model::desired_at(&self.config, previous, &self.control_token(name), name)?;
        let origin = self.native_runtime_home_origin(runtime, home)?;
        // Both generations retain this persisted attachment. Acquire its complete
        // Claim/Sandbox/PVC authority once, then render without additional reads.
        // This proof is local to this call; suspension and final CAS still reread.
        let member = self.pools.prove_native_home(home).await?;
        let before = self
            .pools
            .runtime_desired_for_member(origin, home, previous, name, &member)?;
        model::desired_at(&self.config, next, &self.control_token(name), name)?;
        let desired = self
            .pools
            .runtime_desired_for_member(origin, home, next, name, &member)?;
        Ok((before, desired))
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

    pub(super) async fn wait_suspended(
        &self,
        runtime: &RuntimeHandle,
        desired: &DynamicObject,
    ) -> Result<()> {
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

pub(super) fn validate_successor(
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
    if let Some(reaches) = next.env.get("CHRONO_SANDBOX_REACHES") {
        if !previous.env.contains_key("CHRONO_SANDBOX_REACHES") {
            return Err(rejected("Sandbox resume has no previous service scope"));
        }
        expected
            .env
            .insert("CHRONO_SANDBOX_REACHES".into(), reaches.clone());
        if let (Some(upstream), Some(platform)) = (
            previous.env.get("CHRONO_UPSTREAM_BASE_URL"),
            previous.env.get("CHRONO_PLATFORM_BASE_URL"),
        ) {
            // Reuse the renderer's scope selection; never accept an arbitrary
            // endpoint supplied in the next provider request.
            let reaches: Vec<_> = reaches.split(',').map(str::to_owned).collect();
            expected.env.insert(
                "CHRONO_UPSTREAM_BASE_URL".into(),
                crate::hosted::scoped_model_base_url(upstream, platform, &reaches),
            );
        }
    }
    // The fingerprint covers the new generation and scope. Image, home UID,
    // mounts, tool authority and every other request field must remain exact.
    expected.metadata.remove("cma/spec-sha256");
    let mut submitted = next.clone();
    submitted.metadata.remove("cma/spec-sha256");
    if new <= old || previous.volumes.is_empty() || expected != submitted {
        return Err(rejected(
            "Sandbox resume may change only generation and rendered service scope",
        ));
    }
    Ok(())
}
