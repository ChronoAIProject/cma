use super::{
    api_error, handle, model, parse_handle, rejected, unavailable, AgentSandboxClient,
    SPEC_ANNOTATION,
};
use crate::{
    provider::ProviderSandboxRequest, Result, RuntimeFinalityReceipt, RuntimeHandle,
    RuntimeProviderIncarnation, RuntimeProvisionReceipt,
};
use kube::{
    api::{
        DeleteParams, DynamicObject, ListParams, Patch, PatchParams, PostParams, Preconditions,
        PropagationPolicy,
    },
    ResourceExt,
};
use serde_json::json;
use std::time::Duration;

impl AgentSandboxClient {
    async fn desired(&self, request: &ProviderSandboxRequest) -> Result<DynamicObject> {
        self.desired_at(request, &model::name(request)?).await
    }

    pub(super) async fn desired_at(
        &self,
        request: &ProviderSandboxRequest,
        name: &str,
    ) -> Result<DynamicObject> {
        let desired = model::desired_at(&self.config, request, &self.control_token(name), name)?;
        for volume in &request.volumes {
            let claim = self
                .claims
                .get(&volume.pvc.claim_name)
                .await
                .map_err(api_error)?;
            if let Some(origin) = self.pools.volume_origin(&claim).await? {
                return self.pools.runtime_desired(&origin, request, name).await;
            }
            let allocation: crate::HomeVolumeRequest = claim
                .annotations()
                .get("cma/home-allocation")
                .and_then(|v| serde_json::from_str(v).ok())
                .ok_or_else(|| rejected("PVC has no CMA allocation authority"))?;
            allocation.validate()?;
            if claim.namespace().as_deref() != Some(&self.config.namespace)
                || claim.labels().get("cma/home-managed-by") != Some(&self.config.deployment)
                || claim.metadata.deletion_timestamp.is_some()
                || request.metadata.get("cma/sandbox") != Some(&allocation.sandbox_id)
                || claim.annotations().get("cma/home-request-sha256")
                    != Some(&allocation.computed_sha256())
            {
                return Err(rejected("PVC belongs to another runtime or deployment"));
            }
            let attachment = crate::HomeVolumeAttachment {
                receipt: crate::HomeVolumeReceipt {
                    native_authority: None,
                    volume_id: allocation.volume_id.clone(),
                    request_sha256: allocation.computed_sha256(),
                    storage_reference: claim.name_any(),
                    storage_generation: claim.uid().ok_or_else(|| rejected("PVC has no UID"))?,
                },
                request: allocation,
            };
            if request.metadata.get("cma/home-identity") != Some(&attachment.identity()?) {
                return Err(rejected("PVC UID differs from its admitted home binding"));
            }
        }
        Ok(desired)
    }

    pub(super) fn matches(&self, actual: &DynamicObject, desired: &DynamicObject) -> Result<()> {
        self.owned(actual)?;
        if let Some(encoded) = desired.annotations().get(super::NATIVE_HOME_ANNOTATION) {
            let origin: super::NativePoolHomeOrigin = serde_json::from_str(encoded)
                .map_err(|_| rejected("invalid native runtime origin"))?;
            self.pools.member_owned(actual, &origin.claim)?;
            if actual.uid().as_deref() != Some(&origin.sandbox_uid) {
                return Err(rejected("native Sandbox UID differs from its adopted home"));
            }
        }
        if actual
            .annotations()
            .get(super::pool::runtime_upgrade::UPGRADE_ANNOTATION)
            != desired
                .annotations()
                .get(super::pool::runtime_upgrade::UPGRADE_ANNOTATION)
            || actual.name_any() != desired.name_any()
            || actual.metadata.deletion_timestamp.is_some()
            || desired
                .annotations()
                .iter()
                .any(|(k, v)| actual.annotations().get(k) != Some(v))
            || actual.annotations().get(SPEC_ANNOTATION)
                != desired.annotations().get(SPEC_ANNOTATION)
        {
            return Err(rejected(
                "Sandbox identity conflicts with its creation request",
            ));
        }
        // API defaults may add fields. Require all submitted values recursively;
        // changes to the pinned image/resources/mounts/identity are never adopted.
        let mut admitted = desired.data["spec"].clone();
        // Funding expiry changes independently of the immutable runtime spec.
        // Creation/resume and the backend deadline hook verify it separately.
        if let Some(object) = admitted.as_object_mut() {
            object.remove("shutdownTime");
            object.remove("shutdownPolicy");
        }
        if !super::pool::contains_spec(&actual.data["spec"], &admitted) {
            return Err(crate::AdapterError::AdmittedSandboxSpecDrift);
        }
        Ok(())
    }

    pub async fn create(&self, request: &ProviderSandboxRequest) -> Result<RuntimeHandle> {
        let desired = self.desired(request).await?;
        let name = desired.name_any();
        let object = match self.sandboxes.get_opt(&name).await.map_err(api_error)? {
            Some(object) => object,
            None => match self
                .sandboxes
                .create(&PostParams::default(), &desired)
                .await
            {
                Ok(object) => object,
                Err(error) => self
                    .sandboxes
                    .get_opt(&name)
                    .await
                    .map_err(api_error)?
                    .ok_or_else(|| api_error(error))?,
            },
        };
        self.matches(&object, &desired)?;
        handle(&object)
    }

    pub async fn validate_request(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<DynamicObject>> {
        let (name, _) = parse_handle(runtime)?;
        let desired = self.desired_at(request, name).await?;
        let Some(actual) = self.get(runtime).await? else {
            return Ok(None);
        };
        self.matches(&actual, &desired)?;
        Ok(Some(actual))
    }

    pub async fn wait_ready(
        &self,
        runtime: &RuntimeHandle,
        timeout: Duration,
    ) -> Result<super::PodBinding> {
        tokio::time::timeout(timeout, async {
            let started = tokio::time::Instant::now();
            loop {
                match self.pod_binding(runtime).await {
                    Ok(binding) => return Ok(binding),
                    Err(crate::AdapterError::Unavailable(_)) => {
                        tokio::time::sleep(super::observation::refresh_delay(started.elapsed()))
                            .await
                    }
                    Err(error) => return Err(error),
                }
            }
        })
        .await
        .map_err(|_| unavailable("Sandbox readiness is still pending"))?
    }

    /// The caller coordinates its Agent writers before changing operating mode.
    pub async fn set_suspended(&self, runtime: &RuntimeHandle, suspended: bool) -> Result<()> {
        let object = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Sandbox no longer exists"))?;
        let patch = json!({"metadata":{"uid":object.uid(),"resourceVersion":object.resource_version()},
            "spec":{"operatingMode":if suspended {"Suspended"} else {"Running"}}});
        let updated = self
            .sandboxes
            .patch(
                &object.name_any(),
                &PatchParams::default(),
                &Patch::Merge(patch),
            )
            .await
            .map_err(api_error)?;
        self.owned(&updated)?;
        if updated.uid() != object.uid() {
            return Err(rejected("Sandbox UID changed during update"));
        }
        Ok(())
    }

    pub async fn delete(&self, runtime: &RuntimeHandle) -> Result<()> {
        let object = self.get(runtime).await?;
        self.delete_observed(runtime, object).await
    }

    pub(super) async fn delete_observed(
        &self,
        runtime: &RuntimeHandle,
        object: Option<DynamicObject>,
    ) -> Result<()> {
        let origin = self
            .retirement_pool_origin(runtime, object.as_ref())
            .await?;
        if let Some(origin) = &origin {
            self.pools.delete_claim(&origin.claim).await?;
        }
        if let Some(object) = object {
            match self
                .sandboxes
                .delete(&object.name_any(), &delete_parameters(&object))
                .await
            {
                Ok(_) => {}
                Err(kube::Error::Api(error)) if error.code == 404 => {}
                Err(error) => return Err(api_error(error)),
            }
        }
        // A missing CRD does not prove that its Pod stopped writing to the PV.
        // Retried deletion must establish both absences before home release.
        self.wait_absent(runtime).await?;
        if let Some(origin) = origin {
            self.pools.wait_claim_absent(&origin.claim).await?;
        }
        Ok(())
    }

    pub async fn create_receipted(
        &self,
        request: &ProviderSandboxRequest,
        operation_id: &str,
        request_sha256: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        let desired = self.desired(request).await?;
        let name = desired.name_any();
        let before = self
            .sandboxes
            .list(
                &ListParams::default()
                    .labels(&self.selector())
                    .fields(&format!("metadata.name={name}"))
                    .limit(2),
            )
            .await
            .map_err(api_error)?;
        if !before.items.is_empty() || before.metadata.continue_.is_some_and(|s| !s.is_empty()) {
            return Err(rejected(
                "existing Sandbox cannot acquire a new creation receipt",
            ));
        }
        let before_version = before
            .metadata
            .resource_version
            .ok_or_else(|| unavailable("pre-create observation has no version"))?;
        // Receipts require the successful original response. Read-after-timeout
        // can recover ordinary allocation, but cannot forge a dispatch receipt.
        let object = self
            .sandboxes
            .create(&PostParams::default(), &desired)
            .await
            .map_err(api_error)?;
        self.matches(&object, &desired)?;
        let receipt = RuntimeProvisionReceipt {
            operation_id: operation_id.into(),
            request_sha256: request_sha256.into(),
            provider_response_sha256: crate::provider_request_digest(&object)?,
            handle: handle(&object)?,
            incarnation: self.incarnation(&object)?,
            pre_dispatch_list_version: before_version,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    fn incarnation(&self, object: &DynamicObject) -> Result<RuntimeProviderIncarnation> {
        self.owned(object)?;
        Ok(RuntimeProviderIncarnation {
            scope: self.scope.clone(),
            namespace: self.config.namespace.clone(),
            name: object.name_any(),
            uid: object.uid().ok_or_else(|| rejected("missing UID"))?,
            generation: object.metadata.generation.unwrap_or(1),
            binding_resource_version: object
                .resource_version()
                .ok_or_else(|| rejected("missing resource version"))?,
            identity_sha256: crate::provider_request_digest(&(
                object.annotations(),
                &object.data["spec"],
            ))?,
        })
    }

    pub async fn finalize(
        &self,
        original: &RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        original.validate()?;
        let object = self
            .get(original.handle())
            .await?
            .ok_or_else(|| unavailable("original Sandbox is absent before receipted deletion"))?;
        let mut incarnation = self.incarnation(&object)?;
        incarnation.binding_resource_version =
            original.incarnation.binding_resource_version.clone();
        if incarnation != original.incarnation {
            return Err(rejected("Sandbox incarnation changed before deletion"));
        }
        let response = self
            .sandboxes
            .delete(&object.name_any(), &delete_parameters(&object))
            .await
            .map_err(api_error)?;
        let digest = response.either(
            |v| crate::provider_request_digest(&v),
            |v| crate::provider_request_digest(&v),
        )?;
        let (sandbox_version, pod_version) = self.wait_absent(original.handle()).await?;
        let receipt = RuntimeFinalityReceipt {
            original: original.clone(),
            delete_operation_id: operation_id.into(),
            delete_resource_version: object
                .resource_version()
                .ok_or_else(|| rejected("missing resource version"))?,
            delete_response_sha256: digest,
            absent_runtime_list_version: sandbox_version,
            absent_owned_pods_list_version: pod_version,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub(super) async fn wait_absent(&self, runtime: &RuntimeHandle) -> Result<(String, String)> {
        let (name, uid) = parse_handle(runtime)?;
        tokio::time::timeout(Duration::from_secs(120), async {
            let started = tokio::time::Instant::now();
            loop {
                let objects = self
                    .sandboxes
                    .list(
                        &ListParams::default()
                            .fields(&format!("metadata.name={name}"))
                            .limit(2),
                    )
                    .await
                    .map_err(api_error)?;
                if objects
                    .metadata
                    .continue_
                    .as_ref()
                    .is_some_and(|s| !s.is_empty())
                {
                    return Err(unavailable("deletion observation is incomplete"));
                }
                let original_exists = objects
                    .items
                    .iter()
                    .any(|object| object.uid().as_deref() == Some(uid));
                let pod_absence = if original_exists {
                    None
                } else {
                    self.pools.member_pod_absence_version(uid).await?
                };
                if let Some(pod_version) = pod_absence {
                    return Ok((
                        objects
                            .metadata
                            .resource_version
                            .filter(|version| !version.is_empty())
                            .ok_or_else(|| unavailable("missing Sandbox list version"))?,
                        pod_version,
                    ));
                }
                tokio::time::sleep(super::observation::refresh_delay(started.elapsed())).await;
            }
        })
        .await
        .map_err(|_| unavailable("Sandbox deletion remains pending"))?
    }
}

fn delete_parameters(object: &DynamicObject) -> DeleteParams {
    DeleteParams {
        propagation_policy: Some(PropagationPolicy::Foreground),
        preconditions: Some(Preconditions {
            uid: object.uid(),
            resource_version: object.resource_version(),
        }),
        ..Default::default()
    }
}
