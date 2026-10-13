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
    /// Resolve the pool already recorded on a persisted home attachment. No pool
    /// selection, claim creation or replacement home allocation happens here.
    pub async fn provision_pool_runtime(
        &self,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeHandle>> {
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
        self.admit_pool_runtime(&origin.claim, &home, request)
            .await
            .map(Some)
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
        home.receipt.validate_for(&home.request)?;
        if self.pools.recover_home(original).await?.as_ref() != Some(home)
            || request.metadata.get("cma/runtime-generation")
                != Some(&original.request.runtime_generation.to_string())
        {
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
        let desired = self.pools.runtime_desired(&origin, request, name).await?;
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
        let patch = json!({"metadata":{"uid":actual.uid(),"resourceVersion":actual.resource_version(),
            "labels":{OWNER_LABEL:self.config.deployment},"annotations":desired.annotations()}});
        let updated = self
            .sandboxes
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(api_error)?;
        self.pools.member_owned(&updated, &current)?;
        if updated.uid() != actual.uid() {
            return Err(rejected("native Sandbox UID changed during admission"));
        }
        self.matches(&updated, &desired)?;
        handle(&updated)
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
        self.validate_request(runtime, request)
            .await?
            .ok_or_else(|| unavailable("native Sandbox disappeared before assignment"))?;
        let mut environment = request.env.clone();
        environment.insert(
            "CHRONO_RUNTIME_CONTROL_TOKEN".into(),
            self.control_token(&expected.sandbox_name),
        );
        let bootstrap = RuntimeBootstrapRequest {
            version: RUNTIME_BOOTSTRAP_VERSION,
            pod_uid: expected.pod_uid.clone(),
            environment,
        };
        self.assign_runtime(runtime, expected, &bootstrap)
            .await
            .map(Some)
    }
}
