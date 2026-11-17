//! Native home backup snapshots. This path is independent from legacy home snapshots.

use kube::{
    api::{Api, DynamicObject, PostParams},
    ResourceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::{rejected, AgentSandboxPools, Result};

const MANAGED_LABEL: &str = "cma/native-home-backup-managed-by";
const BACKUP_OF_LABEL: &str = "cma/backup-of";
const BACKUP_ID_LABEL: &str = "cma/backup-id";
const REQUEST_ANNOTATION: &str = "cma/native-home-backup-request";

/// The source identity is frozen before a native backup effect is attempted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHomeSnapshotSource {
    pub namespace: String,
    pub pvc_name: String,
    pub pvc_uid: String,
}

/// Native capture intent. `backup_id` and `backup_of` are also deterministic labels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHomeSnapshotRequest {
    pub backup_id: String,
    pub backup_of: String,
    pub epoch: u64,
    pub snapshot_class: String,
    pub source: NativeHomeSnapshotSource,
}

impl NativeHomeSnapshotRequest {
    pub fn validate(&self) -> Result<()> {
        if self.epoch == 0
            || !label_value(&self.backup_id)
            || !label_value(&self.backup_of)
            || !dns_subdomain(&self.snapshot_class)
            || !dns_label(&self.source.namespace)
            || !dns_subdomain(&self.source.pvc_name)
            || !identity_value(&self.source.pvc_uid)
        {
            return Err(rejected("invalid native home snapshot request"));
        }
        Ok(())
    }

    /// The name is stable for the complete immutable capture intent.
    pub fn deterministic_name(&self) -> String {
        snapshot_name(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHomeSnapshotObservation {
    pub request: NativeHomeSnapshotRequest,
    pub snapshot_name: String,
    pub snapshot_uid: String,
    pub snapshot_content_name: Option<String>,
    pub snapshot_content_uid: Option<String>,
    pub underlying_copy_handle: Option<String>,
    pub creation_time: Option<String>,
    pub ready: bool,
}

impl NativeHomeSnapshotObservation {
    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if self.snapshot_name != self.request.deterministic_name()
            || !identity_value(&self.snapshot_uid)
        {
            return Err(rejected(
                "invalid native home snapshot observation identity",
            ));
        }
        if let Some(value) = &self.creation_time {
            parse_creation_time(value)?;
        }
        match (&self.snapshot_content_name, &self.snapshot_content_uid) {
            (Some(name), Some(uid)) if identity_value(name) && identity_value(uid) => {}
            (None, None) => {}
            _ => return Err(rejected("incomplete native snapshot content identity")),
        }
        if let Some(handle) = &self.underlying_copy_handle {
            if !identity_value(handle) || self.snapshot_content_name.is_none() {
                return Err(rejected("incomplete native snapshot copy evidence"));
            }
        }
        if self.ready
            && (self.creation_time.is_none()
                || self.snapshot_content_name.is_none()
                || self.snapshot_content_uid.is_none()
                || self.underlying_copy_handle.is_none())
        {
            return Err(rejected("ready native snapshot lacks immutable evidence"));
        }
        Ok(())
    }
}

impl AgentSandboxPools {
    /// Create or claim one deterministic VolumeSnapshot, then observe it by name.
    pub async fn create_or_observe_native_home_snapshot(
        &self,
        request: &NativeHomeSnapshotRequest,
    ) -> Result<NativeHomeSnapshotObservation> {
        request.validate()?;
        self.require_native_namespace(request)?;
        let name = request.deterministic_name();
        let existing = self
            .native_snapshots
            .get_opt(&name)
            .await
            .map_err(super::api_error)?;
        let (snapshot, source_guard) = match existing {
            Some(snapshot) => {
                let source_guard = !object_reports_cut(&snapshot)?;
                if source_guard {
                    self.verify_native_source(request).await?;
                }
                (snapshot, source_guard)
            }
            None => {
                // A create is tied to the exact PVC UID both before and after the
                // API effect. A missing post-create GET is a failure, never a recapture.
                self.verify_native_source(request).await?;
                let desired =
                    desired_snapshot(&self.config.namespace, &self.config.deployment, request)?;
                match self
                    .native_snapshots
                    .create(&PostParams::default(), &desired)
                    .await
                {
                    Ok(_) => {}
                    Err(error) if is_already_exists(&error) => {}
                    Err(error) => return Err(super::api_error(error)),
                }
                let snapshot = self
                    .native_snapshots
                    .get(&name)
                    .await
                    .map_err(super::api_error)?;
                self.verify_native_source(request).await?;
                (snapshot, true)
            }
        };
        let observed = observe_snapshot(
            &self.native_snapshot_contents,
            &self.config.namespace,
            &self.config.deployment,
            request,
            &snapshot,
        )
        .await?;
        if source_guard {
            // This check covers both a pending tick and the first capture
            // boundary. Once a ready observation is recorded, later observes
            // are independent of the source PVC's continued existence.
            self.verify_native_source(request).await?;
        }
        observed.validate()?;
        Ok(observed)
    }

    /// Re-observe a previously claimed snapshot. This method never recreates it.
    pub async fn observe_native_home_snapshot(
        &self,
        previous: &NativeHomeSnapshotObservation,
    ) -> Result<NativeHomeSnapshotObservation> {
        previous.validate()?;
        self.require_native_namespace(&previous.request)?;
        // The provider creation time is the immutable capture boundary. Once
        // it has been observed, later PVC disappearance cannot invalidate the
        // snapshot observation.
        let source_guard = source_required_for_observation(previous);
        if source_guard {
            self.verify_native_source(&previous.request).await?;
        }
        let snapshot = self
            .native_snapshots
            .get(&previous.snapshot_name)
            .await
            .map_err(super::api_error)?;
        let observed = observe_snapshot(
            &self.native_snapshot_contents,
            &self.config.namespace,
            &self.config.deployment,
            &previous.request,
            &snapshot,
        )
        .await?;
        if source_guard {
            self.verify_native_source(&previous.request).await?;
        }
        validate_observation_transition(previous, &observed)?;
        Ok(observed)
    }

    fn require_native_namespace(&self, request: &NativeHomeSnapshotRequest) -> Result<()> {
        if request.source.namespace != self.config.namespace {
            return Err(rejected(
                "native home snapshot source is outside CMA namespace",
            ));
        }
        Ok(())
    }

    async fn verify_native_source(&self, request: &NativeHomeSnapshotRequest) -> Result<()> {
        let pvc = self
            .volumes
            .get(&request.source.pvc_name)
            .await
            .map_err(super::api_error)?;
        if pvc.namespace().as_deref() != Some(request.source.namespace.as_str())
            || pvc.uid().as_deref() != Some(request.source.pvc_uid.as_str())
            || pvc.metadata.deletion_timestamp.is_some()
        {
            return Err(rejected("native home snapshot source identity changed"));
        }
        Ok(())
    }
}

#[path = "native_home_snapshot_validation.rs"]
mod validation;
use validation::*;

#[cfg(test)]
#[path = "native_home_snapshots_tests.rs"]
mod tests;
