//! Recover late effects after a worker loses its database lease. Claims and
//! retained PVCs carry durable original authority before orphan cleanup starts.
use kube::{
    api::{Patch, PatchParams},
    ResourceExt,
};
use serde_json::json;

use super::{
    api_error, claims, cleanup_inventory, model, rejected, AgentSandboxPools, NativePoolClaim,
    NativePoolClaimRequest, NativePoolCleanup, NativePoolHomeOrigin, NativePoolMember, Result,
    NATIVE_HOME_ANNOTATION, OWNER_LABEL,
};

const CLEANUP: &str = "cma/native-cleanup";

impl AgentSandboxPools {
    pub async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        let mut result = Vec::new();
        for mut claim in cleanup_inventory::complete(&self.claims).await? {
            if claim.labels().get(OWNER_LABEL) != Some(&self.config.deployment) {
                continue;
            }
            let encoded = claim
                .annotations()
                .get("cma/pool-claim-request")
                .filter(|v| v.len() <= 128 * 1024)
                .ok_or_else(|| rejected("native claim has no allocation request"))?;
            let request: NativePoolClaimRequest = serde_json::from_str(encoded)
                .map_err(|_| rejected("native claim allocation request is invalid"))?;
            claim.metadata.deletion_timestamp = None;
            claims::verify_claim(&claim, &model::claim(&self.config, &request)?)?;
            result.push(claims::receipt(&request, &claim)?);
        }
        Ok(result)
    }

    /// Caller has freshly established that this request has no live database
    /// owner. Freeze discovered member identities on the original claim first.
    pub async fn reclaim_claim(&self, original: &NativePoolClaim) -> Result<()> {
        let Some(actual) = self
            .claims
            .get_opt(&original.name)
            .await
            .map_err(api_error)?
        else {
            return Ok(());
        };
        if actual.uid().as_deref() != Some(&original.uid) {
            return Err(rejected("orphan claim UID changed"));
        }
        let initial = if let Some(encoded) = actual.annotations().get(CLEANUP) {
            let plan: NativePoolCleanup = serde_json::from_str(encoded)
                .map_err(|_| rejected("invalid native cleanup record"))?;
            if plan.request != original.request
                || plan
                    .claim
                    .as_ref()
                    .is_none_or(|claim| claim.uid != original.uid || claim.name != original.name)
            {
                return Err(rejected("native cleanup record belongs to another claim"));
            }
            plan
        } else {
            NativePoolCleanup {
                request: original.request.clone(),
                claim: Some(original.clone()),
                members: vec![],
            }
        };
        let plan = self.plan_cleanup(&initial).await?;
        let encoded =
            serde_json::to_string(&plan).map_err(|_| rejected("invalid native cleanup record"))?;
        if actual.annotations().get(CLEANUP) != Some(&encoded) {
            let patch = json!({"metadata":{"uid":actual.uid(),"resourceVersion":actual.resource_version(),"annotations":{CLEANUP:encoded}}});
            let updated = self
                .claims
                .patch(
                    &original.name,
                    &PatchParams::default(),
                    &Patch::Merge(patch),
                )
                .await
                .map_err(api_error)?;
            if updated.uid() != actual.uid() || updated.annotations().get(CLEANUP) != Some(&encoded)
            {
                return Err(rejected("native cleanup plan was not retained"));
            }
        }
        self.cleanup(&plan).await
    }

    pub async fn cleanup_retained_home(&self, home: &crate::HomeVolumeAttachment) -> Result<()> {
        home.validate_for(&home.request.sandbox_id)?;
        if home.request.policy.is_native_vct() {
            let origin = home
                .receipt
                .native_authority
                .as_deref()
                .ok_or_else(|| rejected("native home authority is missing"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &home.request,
            )?;
            let plan = NativePoolCleanup {
                request: origin.claim.request.clone(),
                claim: Some(origin.claim.clone()),
                members: vec![NativePoolMember {
                    name: origin.claim.sandbox_name.clone().unwrap(),
                    uid: origin.sandbox_uid.clone(),
                    pvc_uid: Some(origin.pvc_uid.clone()),
                }],
            };
            return self.cleanup(&plan).await;
        }
        let Some(volume) = self
            .volumes
            .get_opt(&home.receipt.storage_reference)
            .await
            .map_err(api_error)?
        else {
            return Ok(());
        };
        if volume.uid().as_deref() != Some(&home.receipt.storage_generation) {
            return Err(rejected("retained cleanup volume UID changed"));
        }
        let Some(encoded) = volume.annotations().get(NATIVE_HOME_ANNOTATION) else {
            return Ok(());
        };
        let origin: NativePoolHomeOrigin = serde_json::from_str(encoded)
            .map_err(|_| rejected("invalid native retained home origin"))?;
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &home.request,
        )?;
        if origin.storage_reference()? != home.receipt.storage_reference
            || origin.pvc_uid != home.receipt.storage_generation
        {
            return Err(rejected("native retained home identity changed"));
        }
        let plan = NativePoolCleanup {
            request: origin.claim.request.clone(),
            members: vec![NativePoolMember {
                pvc_uid: None,
                name: origin.claim.sandbox_name.clone().unwrap(),
                uid: origin.sandbox_uid,
            }],
            claim: Some(origin.claim),
        };
        // These identities were recorded on the PVC before detaching it. If an
        // additional member appears, claim reconciliation must persist that plan.
        self.cleanup(&plan).await
    }
}
