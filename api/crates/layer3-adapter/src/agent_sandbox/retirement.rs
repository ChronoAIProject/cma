//! Retirement keeps the original request and home authority across lost replies.
use super::{api_error, parse_handle, rejected, unavailable, AgentSandboxClient};
use crate::{provider::ProviderSandboxRequest, HomeVolumeAttachment, Result, RuntimeHandle};
use kube::ResourceExt;

impl AgentSandboxClient {
    pub(crate) async fn retirement_receiver(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<super::PodBinding> {
        self.validate_request(runtime, request)
            .await?
            .ok_or_else(|| unavailable("retirement runtime is absent"))?;
        self.retirement_home(runtime, home, true).await?;
        self.pod_binding(runtime).await
    }

    pub(crate) async fn retire_verified(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: Option<&crate::RetirementPod>,
    ) -> Result<()> {
        let identity = crate::hosted::provider_request_identity(request)?;
        home.validate_for(&identity.sandbox_id)?;
        if request.metadata.get("cma/home-identity") != Some(&home.identity()?) {
            return Err(rejected(
                "retirement home differs from the original request",
            ));
        }
        let object = self.get(runtime).await?;
        self.retirement_home(runtime, home, object.is_some())
            .await?;
        if let Some(mut actual) = object.clone() {
            let (name, _) = parse_handle(runtime)?;
            let desired = self.desired_at(request, name).await?;
            // An accepted foreground delete is retried with the original UID.
            actual.metadata.deletion_timestamp = None;
            self.matches(&actual, &desired)?;
        }
        if let Some(receiver) = receiver {
            let binding = self.pod_binding(runtime).await?;
            if crate::RetirementPod::try_from(&binding)? != *receiver {
                return Err(rejected("retirement receiver changed before deletion"));
            }
        }
        // Keep the existing UID/resourceVersion preconditions and foreground waiter.
        // The persisted origin also covers a lost reply after CRD/PVC disappearance.
        if let Some(origin) = home.receipt.native_authority.as_deref() {
            self.pools.delete_claim(&origin.claim).await?;
        }
        self.delete_observed(runtime, object).await
    }

    pub(crate) async fn verify_retired(
        &self,
        runtime: &RuntimeHandle,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        if home.request.policy.is_native_vct() {
            self.native_runtime_home_origin(runtime, home)?;
        } else {
            self.retirement_home(runtime, home, false).await?;
        }
        // get rejects a same-name replacement even when the old UID is absent.
        if self.get(runtime).await?.is_some() {
            return Err(unavailable("retiring runtime is still present"));
        }
        self.wait_absent(runtime).await?;
        let origin = match home.receipt.native_authority.as_deref() {
            Some(origin) => Some(origin.clone()),
            None => self.retirement_pool_origin(runtime, None).await?,
        };
        if let Some(origin) = origin {
            if origin.claim.request.home != home.request {
                return Err(rejected("retirement claim differs from original home"));
            }
            self.pools.wait_claim_absent(&origin.claim).await?;
        }
        let (_, member_uid) = parse_handle(runtime)?;
        self.pools
            .prove_retirement_volumes_absent(member_uid, home)
            .await?;
        if self.get(runtime).await?.is_some() {
            return Err(unavailable("runtime appeared during finality observation"));
        }
        Ok(())
    }

    async fn retirement_home(
        &self,
        runtime: &RuntimeHandle,
        home: &HomeVolumeAttachment,
        runtime_present: bool,
    ) -> Result<()> {
        home.validate_for(&home.request.sandbox_id)?;
        let origin = if home.request.policy.is_native_vct() {
            Some(self.native_runtime_home_origin(runtime, home)?)
        } else {
            parse_handle(runtime)?;
            if home
                .request
                .scope
                .as_ref()
                .is_some_and(|scope| scope != &self.legacy_home_scope)
            {
                return Err(rejected(
                    "retirement home belongs to another provider scope",
                ));
            }
            None
        };
        let volume = self
            .claims
            .get_opt(&home.receipt.storage_reference)
            .await
            .map_err(api_error)?;
        let Some(volume) = volume else {
            // A native controller can delete its VCT during foreground retirement.
            // Only the later complete Claim/Sandbox/Pod proof can settle that case.
            if origin.is_some() && !runtime_present {
                return Ok(());
            }
            return Err(unavailable("retirement home authority is missing"));
        };
        if volume.uid().as_deref() != Some(&home.receipt.storage_generation) {
            return Err(rejected("retirement home UID changed"));
        }
        let observed = if let Some(origin) = origin {
            self.pools.native_attachment(origin, &volume)?
        } else {
            HomeVolumeAttachment {
                request: home.request.clone(),
                receipt: crate::provider::PersistentHomeVolumes::native_home_receipt(
                    &self.config.namespace,
                    &self.config.deployment,
                    &home.request,
                    &volume,
                )?,
            }
        };
        if observed != *home {
            return Err(rejected("retirement home identity changed"));
        }
        Ok(())
    }
}
