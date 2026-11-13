//! Health checks precede home adoption; the original Claim always remains pinned.
use super::{api_error, AgentSandboxPools, NativePoolClaim};
use crate::{agent_sandbox::claim_guard, Result};
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, ResourceExt};

impl AgentSandboxPools {
    pub(super) async fn guard_claim_ready(&self, original: &NativePoolClaim) -> Result<()> {
        if !self.config.native_claim_guard_enabled || !original.request.home.policy.is_native_vct()
        {
            return Ok(());
        }
        claim_guard::wait("home", || async {
            let current = self.observe_claim(original).await?;
            let name = current
                .sandbox_name
                .as_deref()
                .ok_or_else(|| super::unavailable("native claim has no assigned Sandbox"))?;
            let selector = super::super::pods::selector(name);
            let parameters = ListParams::default().labels(&selector).limit(3);
            let (member, page) = tokio::join!(self.members.get(name), self.pods.list(&parameters),);
            let member = member.map_err(api_error)?;
            self.member_owned(&member, &current)?;
            let page = page.map_err(api_error)?;
            if page
                .metadata
                .continue_
                .is_some_and(|value| !value.is_empty())
            {
                return Err(super::unavailable("guarded Pod inventory is incomplete"));
            }
            let uid = member.uid().expect("member ownership validated UID");
            let mut owned = Vec::new();
            for value in page.items {
                let pod: Pod = serde_json::from_value(
                    serde_json::to_value(value)
                        .map_err(|_| super::rejected("invalid guarded Pod"))?,
                )
                .map_err(|_| super::rejected("invalid guarded Pod"))?;
                if super::super::model::owned_pod(&pod, &self.config, name, &uid) {
                    owned.push(pod);
                }
            }
            if owned.len() != 1 {
                return Err(super::unavailable(
                    "native claim needs exactly one owned Pod",
                ));
            }
            claim_guard::sample(&member, &owned[0])
        })
        .await
        .map(|_| ())
    }
}
