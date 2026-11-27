//! Non-cloneable proof consumed inside one native adoption/status operation.
use super::*;
use crate::{provider::ProviderSandboxRequest, HomeVolumeAttachment};

pub(crate) struct NativeRuntimeProof {
    member: DynamicObject,
    pod: PodBinding,
    request_sha256: String,
    home_identity: String,
    home_revision: Option<NativeHomeRevision>,
    home_reused: bool,
}
impl NativeRuntimeProof {
    pub(crate) fn member(&self) -> &DynamicObject {
        &self.member
    }
    pub(crate) fn pod(&self) -> &PodBinding {
        &self.pod
    }
    pub(crate) fn home_revision(&self) -> Option<&NativeHomeRevision> {
        self.home_revision.as_ref()
    }
    pub(crate) fn home_reused(&self) -> bool {
        self.home_reused
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
    /// Discover no identity from mutable metadata: the persisted home fixes all
    /// graph names, while the current resources authorize this adoption attempt.
    pub(crate) async fn validate_native_request_with_home(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<DynamicObject> {
        let origin = self.native_runtime_home_origin(runtime, home)?;
        let (name, _) = parse_handle(runtime)?;
        let actual = self.pools.prove_native_home(home).await?;
        let desired = self
            .pools
            .runtime_desired_for_member(origin, home, request, name, &actual)?;
        self.matches(&actual, &desired)?;
        Ok(actual)
    }

    pub(super) fn native_runtime_home_origin<'a>(
        &self,
        runtime: &RuntimeHandle,
        home: &'a HomeVolumeAttachment,
    ) -> Result<&'a NativePoolHomeOrigin> {
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
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &home.request,
        )?;
        Ok(origin)
    }

    pub(crate) async fn verify_native_runtime_policy(
        &self,
        runtime: &RuntimeHandle,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        // Validate the same persisted authority before any speculative policy I/O.
        // The graph proof must still confirm this exact origin on live resources.
        let origin = self.native_runtime_home_origin(runtime, home)?;
        self.pools
            .verify_explicit_template_policy(&origin.claim.request.pool)
            .await
    }

    pub(crate) fn complete_native_runtime_policy(
        &self,
        home: &HomeVolumeAttachment,
        member: &DynamicObject,
        template_policy: Result<()>,
    ) -> Result<()> {
        let origin = home
            .receipt
            .native_authority
            .as_deref()
            .ok_or_else(|| rejected("native home authority missing"))?;
        if self.pools.retained_upgrade_policy(member, origin)? {
            Ok(())
        } else {
            template_policy
        }
    }

    pub(crate) async fn native_runtime_proof(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<NativeRuntimeProof> {
        self.native_runtime_proof_with_revision(runtime, request, home, None)
            .await
    }

    pub(crate) async fn native_runtime_proof_with_revision(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        prior: Option<&NativeHomeRevision>,
    ) -> Result<NativeRuntimeProof> {
        let origin = self.native_runtime_home_origin(runtime, home)?;
        let (name, _) = parse_handle(runtime)?;
        let parameters = ListParams::default().labels(&pods::selector(name)).limit(3);
        let (member, pods) = tokio::join!(
            self.pools.prove_native_home_version(home, prior),
            self.pods.list(&parameters)
        );
        let (member, home_revision, home_reused) = member?;
        let desired = self
            .pools
            .runtime_desired_for_member(origin, home, request, name, &member)?;
        self.matches(&member, &desired)?;
        let pod = self.pod_binding_from_page(&member, pods.map_err(api_error)?)?;
        Ok(NativeRuntimeProof {
            member,
            pod,
            request_sha256: crate::provider_request_digest(request)?,
            home_identity: home.identity()?,
            home_revision,
            home_reused,
        })
    }
}
