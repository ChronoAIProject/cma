//! Non-cloneable proof consumed inside one native adoption/status operation.
use super::*;
use crate::{provider::ProviderSandboxRequest, HomeVolumeAttachment};

pub(crate) struct NativeRuntimeProof {
    member: DynamicObject,
    pod: PodBinding,
    request_sha256: String,
    home_identity: String,
}
impl NativeRuntimeProof {
    pub(crate) fn member(&self) -> &DynamicObject {
        &self.member
    }
    pub(crate) fn pod(&self) -> &PodBinding {
        &self.pod
    }
    pub(crate) fn matches(
        &self,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<bool> {
        Ok(
            self.request_sha256 == crate::provider_request_digest(request)?
                && self.home_identity == home.identity()?,
        )
    }
}
impl AgentSandboxClient {
    pub(crate) async fn native_runtime_proof(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<NativeRuntimeProof> {
        let (name, uid) = parse_handle(runtime)?;
        home.validate_for(&home.request.sandbox_id)?;
        if !home.request.policy.is_native_vct()
            || home.request.scope.as_ref() != Some(&self.native_home_scope)
        {
            return Err(rejected("native VCT belongs to another authority"));
        }
        let origin = home
            .receipt
            .native_authority
            .as_deref()
            .ok_or_else(|| rejected("native home authority missing"))?;
        if origin.claim.sandbox_name.as_deref() != Some(name) || origin.sandbox_uid != uid {
            return Err(rejected("native runtime differs from its persisted home"));
        }
        let parameters = ListParams::default().labels(&pods::selector(name)).limit(3);
        let (member, pods) = tokio::join!(
            self.pools.prove_native_home(home),
            self.pods.list(&parameters)
        );
        let member = member?;
        let desired = self
            .pools
            .runtime_desired_for_home(origin, home, request, name)?;
        self.matches(&member, &desired)?;
        let pod = self.pod_binding_from_page(&member, pods.map_err(api_error)?)?;
        Ok(NativeRuntimeProof {
            member,
            pod,
            request_sha256: crate::provider_request_digest(request)?,
            home_identity: home.identity()?,
        })
    }
}
