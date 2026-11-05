//! Native expiry is a provider control-plane fence, independent of CMA liveness.
use super::{api_error, rejected, AgentSandboxClient};
use crate::{provider::ProviderSandboxRequest, Result, RuntimeHandle};
use kube::{
    api::{Patch, PatchParams},
    ResourceExt,
};
use serde_json::json;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) fn deadline(until: u64) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if until <= now || until > now.saturating_add(330) {
        return Err(rejected(
            "funded deadline is expired or exceeds finite coverage",
        ));
    }
    OffsetDateTime::from_unix_timestamp(
        i64::try_from(until).map_err(|_| rejected("funded deadline overflow"))?,
    )
    .map_err(|_| rejected("funded deadline is invalid"))?
    .format(&Rfc3339)
    .map_err(|_| rejected("funded deadline is invalid"))
}

pub(super) fn apply(spec: &mut serde_json::Value, until: Option<u64>) -> Result<()> {
    if let Some(until) = until {
        spec["shutdownTime"] = json!(deadline(until)?);
        // Core Sandbox Retain removes Pod/Service and retains the Sandbox/PVC.
        // Never set the Claim's similarly named but destructive expiry policy.
        spec["shutdownPolicy"] = json!("Retain");
    }
    Ok(())
}

impl AgentSandboxClient {
    pub async fn set_runtime_deadline(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        until: u64,
    ) -> Result<()> {
        let value = deadline(until)?;
        let actual = self
            .validate_request(runtime, request)
            .await?
            .ok_or_else(|| rejected("funded runtime is absent"))?;
        if let Some(old) = actual.data["spec"]["shutdownTime"].as_str() {
            let old = OffsetDateTime::parse(old, &Rfc3339)
                .map_err(|_| rejected("runtime expiry is invalid"))?;
            if old.unix_timestamp() <= OffsetDateTime::now_utc().unix_timestamp() {
                return Err(rejected("expired runtime generation cannot be extended"));
            }
        }
        if actual.data["spec"]["shutdownTime"] == value
            && actual.data["spec"]["shutdownPolicy"] == "Retain"
        {
            return Ok(());
        }
        let patch = json!({"metadata":{"uid":actual.uid(),"resourceVersion":actual.resource_version()},
            "spec":{"shutdownTime":value,"shutdownPolicy":"Retain"}});
        let updated = self
            .sandboxes
            .patch(
                &actual.name_any(),
                &PatchParams::default(),
                &Patch::Merge(patch),
            )
            .await
            .map_err(api_error)?;
        if updated.uid() != actual.uid()
            || updated.data["spec"]["shutdownTime"] != value
            || updated.data["spec"]["shutdownPolicy"] != "Retain"
        {
            return Err(rejected("provider did not confirm the funded deadline"));
        }
        Ok(())
    }
}
