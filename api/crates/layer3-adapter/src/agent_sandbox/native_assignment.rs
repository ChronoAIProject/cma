//! Fresh native authority proofs immediately before and after bootstrap delivery.
use super::*;
use crate::{
    provider::ProviderSandboxRequest,
    runtime_bootstrap::{RuntimeBootstrapReceipt, RuntimeBootstrapRequest},
    HomeVolumeAttachment, HomeVolumeReceipt,
};

impl AgentSandboxClient {
    pub(super) async fn assign_native_runtime(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        request: &ProviderSandboxRequest,
        bootstrap: &RuntimeBootstrapRequest,
        origin: &NativePoolHomeOrigin,
    ) -> Result<RuntimeBootstrapReceipt> {
        // Reconstruct only the recorded identity. This is not an authority
        // proof: both dispatch boundaries read and validate its live UID graph.
        let home_request = origin.claim.request.home.clone();
        let home = HomeVolumeAttachment {
            receipt: HomeVolumeReceipt {
                native_authority: Some(Box::new(origin.clone())),
                volume_id: home_request.volume_id.clone(),
                request_sha256: home_request.computed_sha256(),
                storage_reference: origin.storage_reference()?,
                storage_generation: origin.pvc_uid.clone(),
            },
            request: home_request,
        };
        tokio::time::timeout(assignment::ASSIGN_TIMEOUT, async {
            let mut timing = super::provision_timing::ProvisionTiming::new();
            // Each proof reads Claim, Sandbox, PVC and Pod inventory concurrently.
            // Nothing is cached across calls or reused across credential delivery.
            let (policy, before) = tokio::join!(
                self.pools
                    .verify_explicit_template_policy(&origin.claim.request.pool),
                async {
                    self.validate_assignment(runtime, expected, bootstrap)?;
                    self.native_runtime_proof(runtime, request, &home).await
                },
            );
            // Original admissions preserve policy error precedence. A proven
            // upgraded member can carry its own retained Pod policy after retirement;
            // neither path delivers bootstrap before complete fresh authority.
            let before = match before {
                Ok(before) => before,
                Err(error) => {
                    if !policy
                        .as_ref()
                        .is_err_and(|error| error.is_native_pool_binding_hold())
                    {
                        policy?;
                    }
                    return Err(error);
                }
            };
            self.complete_native_runtime_policy(&home, before.member(), policy)?;
            if before.pod() != expected {
                return Err(rejected("Sandbox Pod changed before runtime assignment"));
            }
            timing.completed("assignment_before_proof");
            let receipt = self
                .deliver_assignment(&expected.pod_name, bootstrap)
                .await?;
            timing.completed("assignment_delivery");
            let after = self.native_runtime_proof(runtime, request, &home).await?;
            if after.pod() != expected
                || before.member().annotations() != after.member().annotations()
                || before.member().data["spec"] != after.member().data["spec"]
            {
                return Err(rejected("Sandbox changed during runtime assignment"));
            }
            timing.completed("assignment_after_proof");
            Ok(receipt)
        })
        .await
        .map_err(|_| unavailable("runtime assignment timed out"))?
    }
}
