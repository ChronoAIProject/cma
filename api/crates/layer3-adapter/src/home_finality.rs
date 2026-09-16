//! Exact original disk pins and authoritative absence; PVC absence is insufficient.
use crate::{AdapterError, HomeVolumeAttachment, HomeVolumeScope, Result};
use serde::{Deserialize, Serialize};

/// The actual successful original PVC creation response. Reconciliation reads
/// and pre-existing claims cannot produce this acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeProvisionReceipt {
    pub(crate) home: HomeVolumeAttachment,
    pub(crate) operation_id: String,
    pub(crate) creation_resource_version: String,
    pub(crate) creation_response_sha256: String,
}

impl HomeProvisionReceipt {
    pub fn home(&self) -> &HomeVolumeAttachment {
        &self.home
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn validate(&self) -> Result<()> {
        self.home.validate_for(&self.home.request.sandbox_id)?;
        if !crate::provider_finality::identifier(&self.operation_id)
            || !crate::provider_finality::identifier(&self.creation_resource_version)
            || !crate::provider_finality::digest(&self.creation_response_sha256)
        {
            return Err(AdapterError::Rejected(
                "original home creation acknowledgement is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackingReference {
    pub provider_scope: HomeVolumeScope,
    pub home: HomeVolumeAttachment,
    pub pv_name: String,
    pub pv_uid: String,
    pub csi_driver: String,
    /// Exact original provider path, never an inventory selector or name search.
    pub volume_handle: String,
}

impl HomeBackingReference {
    pub fn validate(&self) -> Result<()> {
        self.provider_scope.validate()?;
        self.home.validate_for(&self.home.request.sandbox_id)?;
        if [
            &self.pv_name,
            &self.pv_uid,
            &self.csi_driver,
            &self.volume_handle,
        ]
        .into_iter()
        .any(|v| !crate::provider_finality::identifier(v))
        {
            return Err(AdapterError::Rejected(
                "original home backing identity is unavailable".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackingIdentity {
    pub reference: HomeBackingReference,
    /// Original provider generation, e.g. GKE PD numeric disk ID; captured at creation.
    pub backing_generation: String,
}

impl HomeBackingIdentity {
    pub fn validate(&self) -> Result<()> {
        self.reference.validate()?;
        if !crate::provider_finality::identifier(&self.backing_generation) {
            return Err(AdapterError::Rejected(
                "original backing generation is unavailable".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackingAbsence {
    pub(crate) original: HomeBackingIdentity,
    pub(crate) request_id: String,
    pub(crate) response_sha256: String,
}

impl HomeBackingAbsence {
    pub fn validate(&self) -> Result<()> {
        self.original.validate()?;
        if !crate::provider_finality::identifier(&self.request_id)
            || !crate::provider_finality::digest(&self.response_sha256)
        {
            return Err(AdapterError::Rejected(
                "home backing absence receipt is invalid".into(),
            ));
        }
        Ok(())
    }
}

/// Persisted immediately after the actual UID-preconditioned claim delete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeDeleteReceipt {
    pub(crate) original: HomeBackingIdentity,
    pub(crate) operation_id: String,
    pub(crate) resource_version: String,
    pub(crate) response_sha256: String,
}

impl HomeDeleteReceipt {
    pub fn original(&self) -> &HomeBackingIdentity {
        &self.original
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn validate(&self) -> Result<()> {
        self.original.validate()?;
        if !crate::provider_finality::identifier(&self.operation_id)
            || !crate::provider_finality::identifier(&self.resource_version)
            || !crate::provider_finality::digest(&self.response_sha256)
        {
            return Err(AdapterError::Rejected(
                "home delete acknowledgement is invalid".into(),
            ));
        }
        Ok(())
    }
}

/// A read-only provider implementation must observe this exact original backing.
/// No caller Boolean, current PVC name, or broad disk listing can satisfy it.
#[async_trait::async_trait]
pub trait HomeBackingObserver: Send + Sync {
    /// Observe the exact backing path during original allocation, never retirement.
    async fn capture_original(
        &self,
        reference: &HomeBackingReference,
    ) -> Result<HomeBackingIdentity>;
    async fn observe_original_absence(
        &self,
        original: &HomeBackingIdentity,
    ) -> Result<HomeBackingAbsence>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeFinalityReceipt {
    pub(crate) original: HomeBackingIdentity,
    pub(crate) claim_delete_operation_id: String,
    pub(crate) claim_delete_resource_version: String,
    pub(crate) claim_delete_response_sha256: String,
    pub(crate) backing_absence: HomeBackingAbsence,
}

impl HomeFinalityReceipt {
    pub fn original(&self) -> &HomeBackingIdentity {
        &self.original
    }
    pub fn delete_operation_id(&self) -> &str {
        &self.claim_delete_operation_id
    }
    pub fn delete_receipt(&self) -> HomeDeleteReceipt {
        HomeDeleteReceipt {
            original: self.original.clone(),
            operation_id: self.claim_delete_operation_id.clone(),
            resource_version: self.claim_delete_resource_version.clone(),
            response_sha256: self.claim_delete_response_sha256.clone(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.delete_receipt().validate()?;
        self.backing_absence.validate()?;
        if self.backing_absence.original != self.original {
            return Err(AdapterError::Rejected(
                "home backing absence is for a different original disk".into(),
            ));
        }
        Ok(())
    }
}
