//! Retirement keeps the original request and home authority across lost replies.
use super::{api_error, model, parse_handle, rejected, unavailable, AgentSandboxClient};
use crate::{provider::ProviderSandboxRequest, HomeVolumeAttachment, Result, RuntimeHandle};
use kube::{
    api::{DynamicObject, ListParams},
    ResourceExt,
};

impl AgentSandboxClient {
    pub(crate) async fn verify_retirement_suspension(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.verify_suspended(runtime, request).await?;
        let (_, uid) = parse_handle(runtime)?;
        self.pools.prove_member_pods_absent(uid).await
    }

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

    pub(crate) async fn retirement_probe_pod(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<(String, Option<super::PodBinding>)> {
        let object = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("retirement runtime is absent"))?;
        if object.metadata.deletion_timestamp.is_some() {
            return Err(unavailable("retirement runtime deletion is pending"));
        }
        self.validate_retirement_identity(runtime, request, home, &object)
            .await?;
        self.retirement_home(runtime, home, true).await?;
        self.probe_pod(&object).await
    }

    async fn probe_pod(
        &self,
        object: &DynamicObject,
    ) -> Result<(String, Option<super::PodBinding>)> {
        let page = self
            .pods
            .list(
                &ListParams::default()
                    .labels(&super::pods::selector(&object.name_any()))
                    .limit(3),
            )
            .await
            .map_err(api_error)?;
        let pod = self.owned_pod_from_page(object, page)?;
        let uid = pod
            .uid()
            .filter(|uid| !uid.is_empty())
            .ok_or_else(|| rejected("retirement Pod UID is missing"))?;
        Ok((uid, model::retirement_pod_binding(object, &pod)?))
    }

    pub(crate) async fn retirement_loopback_unavailable(
        &self,
        binding: &super::PodBinding,
    ) -> Result<bool> {
        use kube::api::AttachParams;
        use tokio::io::AsyncReadExt as _;

        // The streamed transport may hide a refused loopback connection behind a
        // generic relay error. This fixed command carries no request or credentials.
        let command = [
            crate::runtime_control_relay::PROGRAM,
            "-I",
            "-u",
            "-c",
            "import socket\ntry:\n with socket.create_connection(('127.0.0.1',9090),timeout=1): print('0')\nexcept (ConnectionRefusedError,ConnectionResetError,TimeoutError): print('1')",
        ];
        let mut exec = self
            .pods
            .exec(
                &binding.pod_name,
                command,
                &AttachParams::default()
                    .container("runtime")
                    .stdout(true)
                    .stderr(false),
            )
            .await
            .map_err(api_error)?;
        let mut bytes = Vec::new();
        exec.stdout()
            .ok_or_else(|| unavailable("retirement socket probe did not open"))?
            .take(3)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| unavailable("retirement socket probe was interrupted"))?;
        let status = exec
            .take_status()
            .ok_or_else(|| unavailable("retirement socket probe status missing"))?;
        if !status
            .await
            .is_some_and(|status| status.status.as_deref() == Some("Success"))
        {
            return Err(unavailable("retirement socket probe did not complete"));
        }
        match bytes.as_slice() {
            b"1\n" => Ok(true),
            b"0\n" => Ok(false),
            _ => Err(rejected("retirement socket probe response is invalid")),
        }
    }

    async fn validate_retirement_identity(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        object: &DynamicObject,
    ) -> Result<()> {
        let (name, uid) = parse_handle(runtime)?;
        let identity = crate::hosted::provider_request_identity(request)?;
        home.validate_for(&identity.sandbox_id)?;
        let home_identity = home.identity()?;
        let annotations = object.annotations();
        if object.name_any() != name
            || object.uid().as_deref() != Some(uid)
            || annotations.get("cma/sandbox") != Some(&identity.sandbox_id)
            || annotations.get("cma/runtime-generation") != Some(&identity.generation.to_string())
            || annotations.get("cma/home-identity") != Some(&home_identity)
            || request.metadata.get("cma/home-identity") != Some(&home_identity)
        {
            return Err(rejected("retirement Sandbox or home identity changed"));
        }
        if home.request.policy.is_native_vct() {
            let origin = self.native_runtime_home_origin(runtime, home)?;
            self.pools.member_owned(object, &origin.claim)?;
        }
        Ok(())
    }

    pub(crate) async fn retire_verified(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: Option<&crate::RetirementPod>,
    ) -> Result<()> {
        self.retire_exact(runtime, request, home, receiver, false)
            .await
    }

    pub(crate) async fn retire_suspended(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        self.retire_exact(runtime, request, home, None, true).await
    }

    async fn retire_exact(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: Option<&crate::RetirementPod>,
        require_suspended: bool,
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
        if let Some(receiver) = receiver.filter(|receiver| receiver.is_uid_only()) {
            if let Some(actual) = &object {
                self.validate_retirement_identity(runtime, request, home, actual)
                    .await?;
                if self.probe_pod(actual).await?.0 != receiver.uid {
                    return Err(rejected("retirement receiver changed before deletion"));
                }
            }
        } else if let Some(mut actual) = object.clone() {
            let (name, _) = parse_handle(runtime)?;
            let mut desired = self.desired_at(request, name).await?;
            if require_suspended {
                desired.data["spec"]["operatingMode"] = serde_json::json!("Suspended");
                if actual
                    .resource_version()
                    .as_deref()
                    .is_none_or(str::is_empty)
                {
                    return Err(rejected("missing retirement resource version"));
                }
            }
            // An accepted foreground delete is retried with the original UID.
            actual.metadata.deletion_timestamp = None;
            self.matches(&actual, &desired)?;
        }
        if require_suspended {
            let (_, uid) = parse_handle(runtime)?;
            self.pools.prove_member_pods_absent(uid).await?;
        }
        if let Some(receiver) = receiver.filter(|receiver| !receiver.is_uid_only()) {
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
