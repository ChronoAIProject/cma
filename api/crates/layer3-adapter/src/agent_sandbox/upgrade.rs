//! Explicit image replacement of a fully stopped native member, under its original UID.
use super::pool::runtime_upgrade::RuntimeUpgradeProof;
use super::*;
use crate::{provider::ProviderSandboxRequest, HomeVolumeAttachment};
use kube::api::{Patch, PatchParams};
use serde_json::json;

impl AgentSandboxClient {
    pub(crate) async fn upgrade_with_home(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        validate_successor(previous, next)?;
        let origin = self.native_runtime_home_origin(runtime, home)?;
        let (name, _) = parse_handle(runtime)?;
        model::desired_at(&self.config, previous, &self.control_token(name), name)?;
        model::desired_at(&self.config, next, &self.control_token(name), name)?;
        let member = self.pools.prove_native_home(home).await?;
        let prior = RuntimeUpgradeProof::from_member(&member, origin)?;
        // A replay proves both exact requests and the original complete home graph.
        // It cannot dispatch another generation after the successor was admitted.
        if let Some(proof) = &prior {
            if proof.is_transition(previous, next)? {
                let desired = self.pools.runtime_desired_with_upgrade(
                    origin,
                    home,
                    next,
                    name,
                    Some(proof),
                )?;
                self.verify_explicit_runtime_policy(&member).await?;
                let actual = self
                    .get(runtime)
                    .await?
                    .ok_or_else(|| unavailable("upgraded Sandbox is absent"))?;
                return self.matches(&actual, &desired);
            }
        }
        let mut before = self
            .pools
            .runtime_desired_for_member(origin, home, previous, name, &member)?;
        before.data["spec"]["operatingMode"] = json!("Suspended");
        self.matches(&member, &before)?;
        let proof = RuntimeUpgradeProof::create(origin, previous, next, prior.as_ref())?;
        let desired =
            self.pools
                .runtime_desired_with_upgrade(origin, home, next, name, Some(&proof))?;
        self.verify_explicit_runtime_policy(&member).await?;
        self.wait_suspended(runtime, &before).await?;
        // Pod removal can take minutes. Reacquire the complete original home
        // graph as well as the stopped generation before the UID/version CAS.
        let actual = self.pools.prove_native_home(home).await?;
        self.matches(&actual, &before)?;
        let version = actual
            .resource_version()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| rejected("Sandbox resource version is absent"))?;
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
            return Err(rejected(
                "Sandbox UID changed during explicit runtime upgrade",
            ));
        }
        self.matches(&updated, &desired)
    }
}

fn validate_successor(
    previous: &ProviderSandboxRequest,
    next: &ProviderSandboxRequest,
) -> Result<()> {
    crate::hosted::validate_runtime_image(&next.image.uri)?;
    // Reuse the strict scope/generation guard, permitting only the image URI and
    // its derived metadata. Labels, mounts, policy and all other fields stay exact.
    let mut allowed = previous.clone();
    allowed.image.uri.clone_from(&next.image.uri);
    allowed.metadata.insert(
        "cma/image-sha256".into(),
        crate::hosted::image_sha256(&next.image.uri),
    );
    super::suspension::validate_successor(&allowed, next)
}
