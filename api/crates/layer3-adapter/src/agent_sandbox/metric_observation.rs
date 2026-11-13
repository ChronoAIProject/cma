//! Namespace-confined GET/list observation. Never adopts, mounts or executes.
use super::{api_error, model, pods, rejected, unavailable, AgentSandboxClient, PodBinding};
use crate::{HomeVolumeAttachment, Result, RuntimeHandle};
use kube::{
    api::{DynamicObject, ListParams},
    ResourceExt,
};

#[derive(Clone, Debug)]
pub struct MetricIdentity {
    pub namespace: String,
    pub deployment: String,
    pub native_sandbox_uid: String,
    pub claim_uid: String,
    pub pvc_name: String,
    pub pvc_uid: String,
    pub runtime: Option<MetricRuntime>,
}
#[derive(Clone, Debug)]
pub struct MetricRuntime {
    pub pod: PodBinding,
    pub generation: u64,
    pub cpu_limit_cores: Option<f64>,
    pub memory_limit_bytes: Option<f64>,
}
impl AgentSandboxClient {
    pub async fn metric_identity(
        &self,
        home: &HomeVolumeAttachment,
        runtime: Option<(&RuntimeHandle, u64)>,
    ) -> Result<MetricIdentity> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            home.validate_for(&home.request.sandbox_id)?;
            let origin = home
                .receipt
                .native_authority
                .as_deref()
                .ok_or_else(|| rejected("unsupported metric identity cohort"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &home.request,
            )?;
            let member = self.pools.prove_native_home(home).await?;
            let mut identity = MetricIdentity {
                namespace: self.config.namespace.clone(),
                deployment: self.config.deployment.clone(),
                native_sandbox_uid: origin.sandbox_uid.clone(),
                claim_uid: origin.claim.uid.clone(),
                pvc_name: home.receipt.storage_reference.clone(),
                pvc_uid: origin.pvc_uid.clone(),
                runtime: None,
            };
            let Some((handle, generation)) = runtime else {
                return Ok(identity);
            };
            // A failed runtime proof must not discard a separately verified
            // retained home. Compute attribution closes at its last proof.
            identity.runtime = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                self.metric_runtime(&member, home, handle, generation),
            )
            .await
            .ok()
            .and_then(Result::ok);
            Ok(identity)
        })
        .await
        .map_err(|_| unavailable("metric identity observation timed out"))?
    }
    async fn metric_runtime(
        &self,
        member: &DynamicObject,
        home: &HomeVolumeAttachment,
        handle: &RuntimeHandle,
        generation: u64,
    ) -> Result<MetricRuntime> {
        let origin = home
            .receipt
            .native_authority
            .as_deref()
            .ok_or_else(|| rejected("native home authority missing"))?;
        let name = member.name_any();
        self.native_runtime_home_origin(handle, home)?;
        if member.annotations().get("cma/sandbox") != Some(&home.request.sandbox_id)
            || member.annotations().get("cma/runtime-generation") != Some(&generation.to_string())
            || generation == 0
        {
            return Err(rejected(
                "metric runtime assignment does not match persisted authority",
            ));
        }
        let page = self
            .pods
            .list(
                &ListParams::default()
                    .labels(&pods::selector(&name))
                    .limit(3),
            )
            .await
            .map_err(api_error)?;
        if page.metadata.continue_.is_some_and(|v| !v.is_empty()) {
            return Err(unavailable("metric Pod observation is incomplete"));
        }
        let mut owned = page
            .items
            .iter()
            .filter(|p| model::owned_pod(p, &self.config, &name, &origin.sandbox_uid));
        let pod = owned
            .next()
            .ok_or_else(|| unavailable("metric runtime Pod absent"))?;
        if owned.next().is_some() {
            return Err(unavailable("ambiguous metric runtime Pod"));
        }
        let binding = model::pod_binding(member, pod)?;
        if binding
            .runtime_container_id
            .as_deref()
            .is_none_or(str::is_empty)
            || binding.runtime_restart_count.is_none()
        {
            return Err(rejected("metric runtime incarnation absent"));
        }
        let spec = pod
            .spec
            .as_ref()
            .ok_or_else(|| rejected("metric Pod spec absent"))?;
        if spec.containers.len() != 1
            || spec.containers[0].name != "runtime"
            || !spec.volumes.as_ref().is_some_and(|vs| {
                vs.iter().any(|v| {
                    v.name == "home"
                        && v.persistent_volume_claim
                            .as_ref()
                            .is_some_and(|p| p.claim_name == home.receipt.storage_reference)
                })
            })
        {
            return Err(rejected("unsupported metric container or home cohort"));
        }
        let limits = spec.containers[0]
            .resources
            .as_ref()
            .and_then(|r| r.limits.as_ref());
        Ok(MetricRuntime {
            pod: binding,
            generation,
            cpu_limit_cores: limits
                .and_then(|l| l.get("cpu"))
                .and_then(|v| quantity(&v.0)),
            memory_limit_bytes: limits
                .and_then(|l| l.get("memory"))
                .and_then(|v| quantity(&v.0)),
        })
    }
}
pub(super) fn quantity(value: &str) -> Option<f64> {
    let (number, multiplier) = [
        ("Ki", 1024.),
        ("Mi", 1_048_576.),
        ("Gi", 1_073_741_824.),
        ("Ti", 1_099_511_627_776.),
        ("m", 0.001),
        ("k", 1000.),
        ("M", 1_000_000.),
        ("G", 1_000_000_000.),
    ]
    .into_iter()
    .find_map(|(suffix, m)| value.strip_suffix(suffix).map(|v| (v, m)))
    .unwrap_or((value, 1.));
    number
        .parse::<f64>()
        .ok()
        .map(|v| v * multiplier)
        .filter(|v| v.is_finite() && *v > 0.)
}
#[cfg(test)]
mod tests {
    #[test]
    fn limits_preserve_units_and_refuse_unknown_denominators() {
        assert_eq!(super::quantity("2000m"), Some(2.));
        assert_eq!(super::quantity("8Gi"), Some(8_589_934_592.));
        for v in ["0", "-2", "NaN", "unknown"] {
            assert_eq!(super::quantity(v), None);
        }
    }
}
