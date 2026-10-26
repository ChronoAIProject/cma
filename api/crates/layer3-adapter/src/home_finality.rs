//! Verified lifecycle of the original namespaced home claim.
//! Physical volume and disk reclamation remain the storage provider's responsibility.
use crate::{AdapterError, HomeVolumeAttachment, HomeVolumeScope, Result};
use serde::{Deserialize, Serialize};

/// The actual successful original PVC POST response. A later GET cannot create it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeProvisionReceipt {
    pub(crate) provider_scope: HomeVolumeScope,
    pub(crate) home: HomeVolumeAttachment,
    pub(crate) operation_id: String,
    pub(crate) creation_resource_version: String,
    pub(crate) creation_response_sha256: String,
}
impl HomeProvisionReceipt {
    pub fn provider_scope(&self) -> &HomeVolumeScope {
        &self.provider_scope
    }
    pub fn home(&self) -> &HomeVolumeAttachment {
        &self.home
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn validate(&self) -> Result<()> {
        self.provider_scope.validate()?;
        self.home.validate_for(&self.home.request.sandbox_id)?;
        if self
            .home
            .request
            .scope
            .as_ref()
            .is_some_and(|scope| scope != &self.provider_scope)
        {
            return Err(AdapterError::Rejected(
                "home creation receipt has another provider scope".into(),
            ));
        }
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

/// Persisted after an acknowledged DELETE fenced by the original PVC UID and current RV.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeDeleteReceipt {
    pub(crate) original: HomeProvisionReceipt,
    pub(crate) operation_id: String,
    pub(crate) resource_version: String,
    pub(crate) response_sha256: String,
}
impl HomeDeleteReceipt {
    pub fn original(&self) -> &HomeProvisionReceipt {
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

/// This receipt makes no claim about a CSI backing volume or physical disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeCleanupScope {
    NamespacedClaim,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeFinalityReceipt {
    pub(crate) delete: HomeDeleteReceipt,
    pub(crate) claim_absence_observation_sha256: String,
    pub(crate) cleanup_scope: HomeCleanupScope,
}
impl HomeFinalityReceipt {
    pub fn original(&self) -> &HomeProvisionReceipt {
        self.delete.original()
    }
    pub fn delete_operation_id(&self) -> &str {
        self.delete.operation_id()
    }
    pub fn delete_receipt(&self) -> HomeDeleteReceipt {
        self.delete.clone()
    }
    pub fn validate(&self) -> Result<()> {
        self.delete.validate()?;
        if self.claim_absence_observation_sha256 != Self::absence_digest(&self.delete)? {
            return Err(AdapterError::Rejected(
                "home claim absence observation does not match its original acknowledgement".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn absence_digest(delete: &HomeDeleteReceipt) -> Result<String> {
        crate::provider_request_digest(&("cma.namespaced-home-claim-absence.v1", delete, 404_u16))
    }
}
