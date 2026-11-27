//! A single read of each known, durable native-home authority resource.
use super::{
    api_error, claims, model, rejected, unavailable, AgentSandboxPools, NativePoolHomeOrigin,
    NATIVE_HOME_ANNOTATION,
};
use crate::{AdapterError, HomeVolumeAttachment, Result};
use kube::{api::DynamicObject, Resource, ResourceExt};

/// A previously validated set of Kubernetes object incarnations and versions.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NativeHomeRevision([(String, String); 3]);

fn revision(object: &impl Resource) -> Option<(String, String)> {
    let metadata = object.meta();
    if metadata.deletion_timestamp.is_some() {
        return None;
    }
    Some((
        metadata.uid.clone().filter(|uid| !uid.is_empty())?,
        metadata
            .resource_version
            .clone()
            .filter(|version| !version.is_empty())?,
    ))
}

impl AgentSandboxPools {
    pub(crate) async fn prove_native_home(
        &self,
        home: &HomeVolumeAttachment,
    ) -> Result<DynamicObject> {
        self.prove_native_home_version(home, None)
            .await
            .map(|(member, _, _)| member)
    }

    pub(crate) async fn prove_native_home_version(
        &self,
        home: &HomeVolumeAttachment,
        prior: Option<&NativeHomeRevision>,
    ) -> Result<(DynamicObject, Option<NativeHomeRevision>, bool)> {
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
        let name = origin
            .claim
            .sandbox_name
            .as_deref()
            .ok_or_else(|| rejected("native Sandbox identity missing"))?;
        let volume_name = origin.storage_reference()?;
        // These names and UIDs come from the persisted attachment, not fresh
        // unvalidated Claim metadata. Preserve Claim/member/volume precedence.
        let (claim, member, volume) = tokio::join!(
            self.claims.get_opt(&origin.claim.name),
            self.members.get_opt(name),
            self.volumes.get_opt(&volume_name),
        );
        let claim = claim
            .map_err(api_error)?
            .ok_or_else(|| unavailable("native claim no longer exists"))?;
        // Equality of UID and resourceVersion proves the live objects still
        // carry the facts validated on the previous read. A missing revision,
        // deletion, replacement or update always goes through full validation.
        let current_revision = revision(&claim).and_then(|claim| {
            let member = revision(member.as_ref().ok()?.as_ref()?)?;
            let volume = revision(volume.as_ref().ok()?.as_ref()?)?;
            Some(NativeHomeRevision([claim, member, volume]))
        });
        if current_revision
            .as_ref()
            .is_some_and(|current| prior == Some(current))
        {
            let member = member
                .map_err(api_error)?
                .ok_or_else(|| unavailable("native Sandbox no longer exists"))?;
            return Ok((member, current_revision, true));
        }
        let desired = model::claim(&self.config, &origin.claim.request)?;
        claims::verify_claim(&claim, &desired)?;
        if origin.claim.name != desired.name_any()
            || claim.uid().as_deref() != Some(&origin.claim.uid)
        {
            return Err(rejected("native claim UID changed"));
        }
        let current = claims::receipt(&origin.claim.request, &claim)?;
        if current.sandbox_name != origin.claim.sandbox_name {
            return Err(rejected("native claim changed its assigned Sandbox"));
        }
        let recorded: NativePoolHomeOrigin = serde_json::from_str(
            claim
                .annotations()
                .get(NATIVE_HOME_ANNOTATION)
                .ok_or_else(|| rejected("native claim home authority missing"))?,
        )
        .map_err(|_| rejected("invalid native home authority"))?;
        if recorded != *origin {
            return Err(rejected("native claim home authority changed"));
        }
        let member = member
            .map_err(api_error)?
            .ok_or_else(|| unavailable("native Sandbox no longer exists"))?;
        self.member_owned(&member, &origin.claim)?;
        let drifted = match self.member_spec(&member, origin, false) {
            Ok(true) => false,
            Ok(false) => return Err(rejected("native Sandbox home authority changed")),
            Err(AdapterError::AdmittedSandboxSpecDrift) => true,
            Err(error) => return Err(error),
        };
        let volume = volume
            .map_err(api_error)?
            .ok_or_else(|| unavailable("native PVC no longer exists"))?;
        if self.native_attachment(origin, &volume)? != *home {
            return Err(rejected("native home identity changed"));
        }
        if drifted {
            return Err(AdapterError::AdmittedSandboxSpecDrift);
        }
        Ok((member, current_revision, false))
    }
}
