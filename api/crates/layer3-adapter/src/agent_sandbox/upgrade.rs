//! Explicit image replacement of a fully stopped native member, under its original UID.
use super::pool::runtime_upgrade::{RetainedStartupPolicyProof, RuntimeUpgradeProof};
use super::*;
use crate::{
    provider::ProviderSandboxRequest, HomeVolumeAttachment, RetainedStartupPolicyPredecessor,
    RetainedStartupPolicyUpgrade,
};
use kube::api::{Patch, PatchParams};
use serde_json::json;

impl AgentSandboxClient {
    /// Observe the exact stopped member and original home without admitting work.
    pub async fn observe_retained_startup_policy_predecessor(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<RetainedStartupPolicyPredecessor> {
        let origin = self.native_runtime_home_origin(runtime, home)?;
        let (name, uid) = parse_handle(runtime)?;
        let member = self.pools.prove_native_home(home).await?;
        let mut desired = self
            .pools
            .runtime_desired_for_member(origin, home, previous, name, &member)?;
        desired.data["spec"]["operatingMode"] = json!("Suspended");
        self.matches(&member, &desired)?;
        self.verify_explicit_runtime_policy(&member).await?;
        let resource_version = member
            .resource_version()
            .filter(|version| !version.is_empty())
            .ok_or_else(|| rejected("Sandbox resource version is absent"))?;
        self.pools.prove_member_pods_absent(uid).await?;
        let current = self.pools.prove_native_home(home).await?;
        if current.resource_version().as_ref() != Some(&resource_version) {
            return Err(unavailable("stopped Sandbox changed during observation"));
        }
        self.matches(&current, &desired)?;
        retained_predecessor(uid, &resource_version, previous, home)
    }

    pub(crate) async fn upgrade_with_home(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        validate_successor(previous, next)?;
        self.upgrade_stopped_member(runtime, previous, next, home, None)
            .await
    }

    /// Apply only the admission's frozen predecessor or observe its exact replay.
    pub async fn upgrade_retained_startup_policy(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        admission: &RetainedStartupPolicyUpgrade<'_>,
    ) -> Result<()> {
        let (_, uid) = parse_handle(runtime)?;
        let resource_version = &admission.predecessor.resource_version;
        if resource_version.is_empty()
            || retained_predecessor(uid, resource_version, previous, home)?
                != *admission.predecessor
        {
            return Err(rejected(
                "retained startup policy predecessor differs from admission",
            ));
        }
        let next = previous.retained_startup_policy_successor(
            admission.target_image,
            admission.successor_generation,
            admission.journal_recovery,
        )?;
        self.upgrade_stopped_member(runtime, previous, &next, home, Some(admission))
            .await
    }

    async fn upgrade_stopped_member(
        &self,
        runtime: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        admission: Option<&RetainedStartupPolicyUpgrade<'_>>,
    ) -> Result<()> {
        let origin = self.native_runtime_home_origin(runtime, home)?;
        let (name, _) = parse_handle(runtime)?;
        model::desired_at(&self.config, previous, &self.control_token(name), name)?;
        model::desired_at(&self.config, next, &self.control_token(name), name)?;
        let member = self.pools.prove_native_home(home).await?;
        let prior = RuntimeUpgradeProof::from_member(&member, origin)?;
        let policy = admission.map(|admission| RetainedStartupPolicyProof {
            operation_id: admission.operation_id.into(),
            predecessor_resource_version: admission.predecessor.resource_version.clone(),
            journal_recovery: admission.journal_recovery,
        });
        // A replay proves both exact requests and the original complete home graph.
        // It cannot dispatch another generation after the successor was admitted.
        if let Some(proof) = &prior {
            if proof.retained_policy() == policy.as_ref() && proof.is_transition(previous, next)? {
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
        if admission.is_none()
            && prior
                .as_ref()
                .and_then(RuntimeUpgradeProof::retained_policy)
                .is_some()
        {
            return Err(rejected(
                "retained startup policy requires a scoped runtime transition",
            ));
        }
        let mut before = self
            .pools
            .runtime_desired_for_member(origin, home, previous, name, &member)?;
        before.data["spec"]["operatingMode"] = json!("Suspended");
        self.matches(&member, &before)?;
        let proof = RuntimeUpgradeProof::create(origin, previous, next, prior.as_ref(), policy)?;
        let desired =
            self.pools
                .runtime_desired_with_upgrade(origin, home, next, name, Some(&proof))?;
        self.verify_explicit_runtime_policy(&member).await?;
        if let Some(admission) = admission {
            self.pools
                .prove_member_pods_absent(&admission.predecessor.uid)
                .await?;
        } else {
            self.wait_suspended(runtime, &before).await?;
        }
        // Pod removal can take minutes. Reacquire the complete original home
        // graph as well as the stopped generation before the UID/version CAS.
        let actual = self.pools.prove_native_home(home).await?;
        self.matches(&actual, &before)?;
        let version = actual
            .resource_version()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| rejected("Sandbox resource version is absent"))?;
        if admission.is_some_and(|admission| {
            version != admission.predecessor.resource_version
                || actual.uid().as_deref() != Some(admission.predecessor.uid.as_str())
        }) {
            return Err(rejected(
                "retained startup policy predecessor changed after admission",
            ));
        }
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

fn retained_predecessor(
    uid: &str,
    resource_version: &str,
    request: &ProviderSandboxRequest,
    home: &HomeVolumeAttachment,
) -> Result<RetainedStartupPolicyPredecessor> {
    let identity = crate::hosted::provider_request_identity(request)?;
    let journal_recovery = match request
        .env
        .get(crate::hosted::JOURNAL_RECOVERY)
        .map(String::as_str)
    {
        None | Some("0" | "false") => false,
        Some("1" | "true") => true,
        Some(_) => return Err(rejected("retained startup policy is invalid")),
    };
    Ok(RetainedStartupPolicyPredecessor {
        uid: uid.into(),
        resource_version: resource_version.into(),
        request_sha256: crate::provider_request_digest(request)?,
        home_identity: home.identity()?,
        image: request.image.uri.clone(),
        generation: identity.generation,
        journal_recovery,
    })
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
