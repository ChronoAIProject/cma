//! Durable home allocation requests; physical storage belongs to the adapter.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AdapterError, Result};

/// Opaque configured provider scope, captured before any physical allocation.
/// Its digest never exposes provider endpoints or credentials to domain storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HomeVolumeScope(String);

impl HomeVolumeScope {
    pub fn from_authority(provider: &str, components: &[&str]) -> Result<Self> {
        if !identifier(provider)
            || components.is_empty()
            || components.iter().any(|value| {
                value.is_empty() || value.len() > 8_192 || value.chars().any(char::is_control)
            })
        {
            return Err(invalid("home provider scope is unavailable"));
        }
        let mut digest = Sha256::new();
        digest.update(b"cma-home-provider-scope-v1\0");
        for value in std::iter::once(provider).chain(components.iter().copied()) {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Ok(Self(format!("{:x}", digest.finalize())))
    }

    pub fn validate(&self) -> Result<()> {
        if self.0.len() != 64
            || !self
                .0
                .bytes()
                .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
        {
            return Err(invalid("home provider scope is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeVolumePolicy {
    pub storage_class: String,
    pub capacity_gib: u32,
    pub template_name: String,
    pub template_generation: String,
}

impl HomeVolumePolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=1_024).contains(&self.capacity_gib)
            || !identifier(&self.storage_class)
            || !identifier(&self.template_name)
            || !identifier(&self.template_generation)
        {
            return Err(invalid("home storage policy is invalid"));
        }
        Ok(())
    }
}

/// Must be recorded before allocation. A retry keeps the same identity and policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeVolumeRequest {
    pub volume_id: String,
    pub sandbox_id: String,
    pub policy: HomeVolumePolicy,
    /// Recorded before allocation; absent historical requests belong to the
    /// original local store, never whichever provider is the current default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<HomeVolumeScope>,
}

impl HomeVolumeRequest {
    pub fn validate(&self) -> Result<()> {
        self.policy.validate()?;
        if let Some(scope) = &self.scope {
            scope.validate()?;
        }
        if !identifier(&self.volume_id) || !identifier(&self.sandbox_id) {
            return Err(invalid("home allocation lineage is invalid"));
        }
        Ok(())
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"cma-home-volume-v1\0");
        for value in [
            &self.volume_id,
            &self.sandbox_id,
            &self.policy.storage_class,
            &self.policy.template_name,
            &self.policy.template_generation,
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        digest.update(self.policy.capacity_gib.to_be_bytes());
        // Preserve every historical request digest exactly when scope is absent.
        if let Some(scope) = &self.scope {
            digest.update(b"cma-home-request-scope-v1\0");
            digest.update(scope.0.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }
}

/// Opaque physical identity. Reusing a reference cannot replace its generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeVolumeReceipt {
    pub volume_id: String,
    pub request_sha256: String,
    pub storage_reference: String,
    pub storage_generation: String,
}

impl HomeVolumeReceipt {
    pub fn validate_for(&self, request: &HomeVolumeRequest) -> Result<()> {
        request.validate()?;
        if self.volume_id != request.volume_id
            || self.request_sha256 != request.computed_sha256()
            || !identifier(&self.storage_reference)
            || !identifier(&self.storage_generation)
        {
            return Err(invalid(
                "home storage receipt does not match its allocation",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeVolumeAttachment {
    pub request: HomeVolumeRequest,
    pub receipt: HomeVolumeReceipt,
}

impl HomeVolumeAttachment {
    pub fn identity(&self) -> Result<String> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        self.receipt.validate_for(&self.request)?;
        let encoded = serde_json::to_vec(self)
            .map_err(|_| invalid("durable home identity cannot be encoded"))?;
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(encoded)))
    }

    pub fn validate_for(&self, sandbox_id: &str) -> Result<()> {
        self.receipt.validate_for(&self.request)?;
        if self.request.sandbox_id != sandbox_id {
            return Err(invalid("home storage belongs to another sandbox"));
        }
        Ok(())
    }
}

/// Control-plane storage effects. Runtime teardown alone never releases a home.
#[async_trait::async_trait]
pub trait HomeVolumeStore: Send + Sync {
    async fn delete_original_home(
        &self,
        _original: &crate::HomeProvisionReceipt,
        _operation_id: &str,
    ) -> Result<crate::HomeDeleteReceipt> {
        Err(AdapterError::Unsupported(
            "exact original home deletion is unavailable".into(),
        ))
    }
    async fn observe_home_finality(
        &self,
        _delete: &crate::HomeDeleteReceipt,
    ) -> Result<crate::HomeFinalityReceipt> {
        Err(AdapterError::Unsupported(
            "original home claim absence is unavailable".into(),
        ))
    }
    async fn prepare_receipted(
        &self,
        _request: &HomeVolumeRequest,
        _operation_id: &str,
    ) -> Result<crate::HomeProvisionReceipt> {
        Err(AdapterError::Unsupported(
            "original home creation acknowledgement is unavailable".into(),
        ))
    }
    /// Optional evidence capability. Absence never grants deletion provenance.
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        None
    }
    /// Complete, validated inventory in this controller's storage scope.
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>>;
    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt>;
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> Result<()>;
    /// Release snapshot/export resources after all capture workers have stopped.
    /// Providers without disk snapshot support have no dependent resources.
    async fn remove_captures(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        attachment.receipt.validate_for(&attachment.request)
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<()>;
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn invalid(detail: &str) -> AdapterError {
    AdapterError::Rejected(detail.into())
}
