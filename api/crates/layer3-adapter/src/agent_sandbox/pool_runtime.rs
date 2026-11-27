use kube::{
    api::{Patch, PatchParams},
    ResourceExt,
};
use serde_json::json;

use super::{
    api_error, handle, pool, rejected, unavailable, AgentSandboxClient, NativePoolClaim,
    NativePoolHomeOrigin, PodBinding, NATIVE_HOME_ANNOTATION, OWNER_LABEL,
};
use crate::{
    provider::ProviderSandboxRequest,
    runtime_bootstrap::{
        RuntimeBootstrapReceipt, RuntimeBootstrapRequest, RUNTIME_BOOTSTRAP_VERSION,
    },
    HomeVolumeAttachment, Result, RuntimeHandle,
};

impl AgentSandboxClient {
    /// Persisted native VCT identity replaces only PVC/member origin discovery.
    /// Admission still reads the complete live graph and fences its publication.
    pub(crate) async fn provision_pool_runtime_with_home(
        &self,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<RuntimeHandle> {
        request.validate_volumes()?;
        let identity = crate::hosted::provider_request_identity(request)?;
        home.validate_for(&identity.sandbox_id)?;
        if !home.request.policy.is_native_vct()
            || home.request.scope.as_ref() != Some(&self.native_home_scope)
            || request.metadata.get("cma/home-identity") != Some(&home.identity()?)
        {
            return Err(rejected(
                "native runtime requires its persisted home authority",
            ));
        }
        let origin = home
            .receipt
            .native_authority
            .as_deref()
            .ok_or_else(|| rejected("native home authority missing"))?;
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &home.request,
        )?;
        self.admit_pool_runtime(&origin.claim, home, request).await
    }

    /// Resolve the pool already recorded on a persisted home attachment. No pool
    /// selection, claim creation or replacement home allocation happens here.
    pub async fn provision_pool_runtime(
        &self,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeHandle>> {
        let mut timing = super::provision_timing::ProvisionTiming::new();
        request.validate_volumes()?;
        let Some(first) = request.volumes.first() else {
            return Ok(None);
        };
        let volume = self
            .claims
            .get(&first.pvc.claim_name)
            .await
            .map_err(api_error)?;
        let Some(origin) = self.pools.volume_origin(&volume).await? else {
            return Ok(None);
        };
        let home = if origin.claim.request.home.policy.is_native_vct() {
            self.pools.native_attachment(&origin, &volume)?
        } else {
            HomeVolumeAttachment {
                request: origin.claim.request.home.clone(),
                receipt: crate::provider::PersistentHomeVolumes::native_home_receipt(
                    &self.config.namespace,
                    &self.config.deployment,
                    &origin.claim.request.home,
                    &volume,
                )?,
            }
        };
        if request.metadata.get("cma/home-identity") != Some(&home.identity()?) {
            return Err(rejected(
                "native runtime requires the persisted home attachment",
            ));
        }
        timing.completed("persisted_home_origin");
        let handle = self
            .admit_pool_runtime(&origin.claim, &home, request)
            .await?;
        timing.completed("admit_pool_runtime");
        Ok(Some(handle))
    }

    pub(super) async fn retirement_pool_origin(
        &self,
        runtime: &RuntimeHandle,
        actual: Option<&kube::api::DynamicObject>,
    ) -> Result<Option<NativePoolHomeOrigin>> {
        let (name, uid) = super::parse_handle(runtime)?;
        let encoded = if let Some(actual) = actual {
            actual.annotations().get(NATIVE_HOME_ANNOTATION).cloned()
        } else {
            // A lost foreground delete reply may leave only the retained PVC.
            // Its origin pins the original claim even after the CRD is gone.
            let Some(volume) = self
                .claims
                .get_opt(&format!("home-{name}"))
                .await
                .map_err(api_error)?
            else {
                return Ok(None);
            };
            let Some(encoded) = volume.annotations().get(NATIVE_HOME_ANNOTATION) else {
                return Ok(None);
            };
            let origin: NativePoolHomeOrigin = serde_json::from_str(encoded)
                .map_err(|_| rejected("invalid native retirement origin"))?;
            crate::provider::PersistentHomeVolumes::native_home_receipt(
                &self.config.namespace,
                &self.config.deployment,
                &origin.claim.request.home,
                &volume,
            )?;
            Some(encoded.clone())
        };
        let Some(encoded) = encoded else {
            return Ok(None);
        };
        let origin: NativePoolHomeOrigin = serde_json::from_str(&encoded)
            .map_err(|_| rejected("invalid native retirement origin"))?;
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &origin.claim.request.home,
        )?;
        if origin.sandbox_uid != uid || origin.claim.sandbox_name.as_deref() != Some(name) {
            return Err(rejected(
                "native retirement origin belongs to another Sandbox",
            ));
        }
        if let Some(actual) = actual {
            // A terminating CRD may still require repeated UID-fenced cleanup.
            let mut observed = actual.clone();
            observed.metadata.deletion_timestamp = None;
            self.pools.member_owned(&observed, &origin.claim)?;
        }
        Ok(Some(origin))
    }

    /// The caller must persist both the claim receipt and home attachment first.
    /// This only publishes runtime identity; it does not mutate the Pod template
    /// or send a credential to the waiting process.
    pub async fn admit_pool_runtime(
        &self,
        original: &NativePoolClaim,
        home: &HomeVolumeAttachment,
        request: &ProviderSandboxRequest,
    ) -> Result<RuntimeHandle> {
        let mut timing = super::provision_timing::ProvisionTiming::new();
        home.receipt.validate_for(&home.request)?;
        let (current, actual) = if home.request.policy.is_native_vct() {
            home.validate_for(&home.request.sandbox_id)?;
            let origin = home
                .receipt
                .native_authority
                .as_deref()
                .ok_or_else(|| rejected("native home authority missing"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &home.request,
            )?;
            if home.request.scope.as_ref() != Some(&self.native_home_scope)
                || original.name != origin.claim.name
                || original.uid != origin.claim.uid
                || original.request != origin.claim.request
                || original
                    .sandbox_name
                    .as_ref()
                    .is_some_and(|name| Some(name) != origin.claim.sandbox_name.as_ref())
                || request.metadata.get("cma/runtime-generation")
                    != Some(&original.request.runtime_generation.to_string())
            {
                return Err(rejected(
                    "native runtime requires its original persisted claim/home",
                ));
            }
            self.pools
                .validate_runtime_home_request(origin, home, request)?;
            // Both proofs are fresh and read-only at this admission boundary.
            // Preserve policy error precedence and the member version fence.
            let (policy, actual) = tokio::join!(
                self.pools
                    .verify_explicit_template_policy(&original.request.pool),
                self.pools.prove_native_home(home),
            );
            if policy
                .as_ref()
                .is_err_and(|error| error.is_native_pool_binding_hold())
            {
                if let Err(error) = actual.as_ref() {
                    return Err(error.clone());
                }
            }
            policy?;
            let actual = actual?;
            timing.completed("admission_policy_and_home_proof");
            (origin.claim.clone(), actual)
        } else {
            self.pools
                .verify_explicit_template_policy(&original.request.pool)
                .await?;
            timing.completed("admission_template_policy");
            if request.metadata.get("cma/runtime-generation")
                != Some(&original.request.runtime_generation.to_string())
            {
                return Err(rejected(
                    "native runtime requires its original persisted claim/home",
                ));
            }
            if self.pools.recover_home(original).await?.as_ref() != Some(home) {
                return Err(rejected(
                    "native runtime requires its original persisted claim/home",
                ));
            }
            let current = self.pools.observe_claim(original).await?;
            let name = current
                .sandbox_name
                .as_deref()
                .ok_or_else(|| unavailable("native claim is not assigned"))?;
            let actual = self.sandboxes.get(name).await.map_err(api_error)?;
            self.pools.member_owned(&actual, &current)?;
            timing.completed("admission_home_proof");
            (current, actual)
        };
        let name = current
            .sandbox_name
            .as_deref()
            .ok_or_else(|| unavailable("native claim is not assigned"))?;
        let origin: NativePoolHomeOrigin = serde_json::from_str(
            actual
                .annotations()
                .get(NATIVE_HOME_ANNOTATION)
                .ok_or_else(|| rejected("native home was not adopted"))?,
        )
        .map_err(|_| rejected("invalid native home origin"))?;
        if origin.claim.uid != current.uid
            || origin.claim.request != current.request
            || origin.claim.sandbox_name != current.sandbox_name
            || origin.pvc_uid != home.receipt.storage_generation
            || !self.pools.member_spec(&actual, &origin, true)?
        {
            return Err(rejected(
                "native runtime has conflicting adoption authority",
            ));
        }
        let desired = if home.request.policy.is_native_vct() {
            self.pools
                .runtime_desired_for_home(&origin, home, request, name)?
        } else {
            self.pools.runtime_desired(&origin, request, name).await?
        };
        if actual
            .annotations()
            .contains_key(pool::RUNTIME_REQUEST_ANNOTATION)
        {
            self.matches(&actual, &desired)?;
            return handle(&actual);
        }
        if !super::model::ready(&actual) {
            return Err(unavailable("native pool member is not ready"));
        }
        if actual
            .labels()
            .get(OWNER_LABEL)
            .is_some_and(|v| v != &self.config.deployment)
            || request
                .metadata
                .keys()
                .any(|k| actual.annotations().contains_key(k))
            || actual.annotations().contains_key(super::SPEC_ANNOTATION)
            || actual.data["spec"]["operatingMode"]
                .as_str()
                .is_some_and(|mode| mode != "Running")
        {
            return Err(rejected("native pool member already has runtime authority"));
        }
        let mut patch = json!({"metadata":{"uid":actual.uid(),"resourceVersion":actual.resource_version(),
            "labels":{OWNER_LABEL:self.config.deployment},"annotations":desired.annotations()}});
        if request.funded_until.is_some() {
            patch["spec"] = json!({"shutdownTime":desired.data["spec"]["shutdownTime"],"shutdownPolicy":"Retain"});
        }
        timing.completed("admission_render_validate");
        let updated = self
            .sandboxes
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(api_error)?;
        timing.completed("admission_publish");
        self.pools.member_owned(&updated, &current)?;
        if updated.uid() != actual.uid() {
            return Err(rejected("native Sandbox UID changed during admission"));
        }
        self.matches(&updated, &desired)?;
        handle(&updated)
    }

    /// Recheck Template authority only before transitions that admit work or keys.
    pub(crate) async fn verify_explicit_runtime_policy(
        &self,
        object: &kube::api::DynamicObject,
    ) -> Result<()> {
        if let Some(encoded) = object.annotations().get(NATIVE_HOME_ANNOTATION) {
            let origin: NativePoolHomeOrigin = serde_json::from_str(encoded)
                .map_err(|_| rejected("invalid native runtime origin"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &origin.claim.request.home,
            )?;
            if !self.pools.retained_upgrade_policy(object, &origin)? {
                self.pools
                    .verify_explicit_template_policy(&origin.claim.request.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Called by the hosted backend before binding transport on initial creation,
    /// adoption or resume. Ordinary cold runtimes already carry their environment.
    pub async fn assign_pool_runtime(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeBootstrapReceipt>> {
        let object = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Sandbox disappeared before runtime binding"))?;
        if !object.annotations().contains_key(NATIVE_HOME_ANNOTATION) {
            return Ok(None);
        }
        let origin: NativePoolHomeOrigin =
            serde_json::from_str(&object.annotations()[NATIVE_HOME_ANNOTATION])
                .map_err(|_| rejected("invalid native runtime origin"))?;
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &origin.claim.request.home,
        )?;
        if !origin.claim.request.home.policy.is_native_vct() {
            self.verify_explicit_runtime_policy(&object).await?;
        }
        let mut environment = request.env.clone();
        environment.insert(
            "CHRONO_RUNTIME_CONTROL_TOKEN".into(),
            self.control_token(&expected.sandbox_name),
        );
        let staging_startup_failure = self
            .staging_startup_failure
            .as_ref()
            .filter(|fault| {
                origin.claim.request.home.policy.is_native_vct()
                    && self.runtime_startup_breaker_enabled()
                    && fault.targets_assignment(
                        &origin.claim.request.home.sandbox_id,
                        expected.runtime_restart_count,
                        std::time::SystemTime::now(),
                    )
            })
            .cloned();
        let bootstrap = RuntimeBootstrapRequest {
            version: RUNTIME_BOOTSTRAP_VERSION,
            pod_uid: expected.pod_uid.clone(),
            environment,
            staging_startup_failure,
        };
        if origin.claim.request.home.policy.is_native_vct() {
            return self
                .assign_native_runtime(runtime, expected, request, &bootstrap, &origin)
                .await
                .map(Some);
        }
        self.validate_request(runtime, request)
            .await?
            .ok_or_else(|| unavailable("native Sandbox disappeared before assignment"))?;
        self.assign_runtime(runtime, expected, &bootstrap)
            .await
            .map(Some)
    }
}
