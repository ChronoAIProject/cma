//! Home storage outlives a runtime and transfers with a warm reservation.

use layer3_adapter::{
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest,
};
use serde::{Deserialize, Serialize};

use crate::SandboxId;

use super::SandboxModelError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHome {
    pub request: HomeVolumeRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<HomeVolumeReceipt>,
    #[serde(default)]
    pub released: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_pool: Option<Box<super::SandboxNativePool>>,
}

impl SandboxHome {
    pub fn reserve(id: &SandboxId, policy: HomeVolumePolicy) -> Result<Self, SandboxModelError> {
        let home = Self {
            request: HomeVolumeRequest {
                scope: None,
                volume_id: format!("home_{}", uuid::Uuid::new_v4().simple()),
                sandbox_id: id.to_string(),
                policy,
            },
            receipt: None,
            released: false,
            native_pool: None,
        };
        home.validate(id)?;
        Ok(home)
    }

    pub fn validate(&self, id: &SandboxId) -> Result<(), SandboxModelError> {
        self.request.validate().map_err(|_| invalid())?;
        if self.request.sandbox_id != id.as_str() {
            return Err(invalid());
        }
        if let Some(receipt) = &self.receipt {
            receipt.validate_for(&self.request).map_err(|_| invalid())?;
        }
        if let Some(native) = &self.native_pool {
            native.validate_for(&self.request)?;
            if self.released && !native.cleaned {
                return Err(invalid());
            }
            if let Some(receipt) = &self.receipt {
                let name = native
                    .claim
                    .as_ref()
                    .and_then(|claim| claim.sandbox_name.as_ref())
                    .ok_or_else(invalid)?;
                if receipt.storage_reference != format!("home-{name}") {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    pub fn bind(&mut self, receipt: HomeVolumeReceipt) -> Result<(), SandboxModelError> {
        receipt.validate_for(&self.request).map_err(|_| invalid())?;
        if self.released
            || self
                .receipt
                .as_ref()
                .is_some_and(|current| current != &receipt)
        {
            return Err(invalid());
        }
        if let Some(native) = &self.native_pool {
            native.validate_for(&self.request)?;
            if native.cleaned
                || native.cleanup.is_some()
                || native
                    .claim
                    .as_ref()
                    .and_then(|c| c.sandbox_name.as_ref())
                    .is_none_or(|name| receipt.storage_reference != format!("home-{name}"))
            {
                return Err(invalid());
            }
        }
        self.receipt = Some(receipt);
        Ok(())
    }

    pub fn attachment(&self) -> Option<HomeVolumeAttachment> {
        if self.released {
            return None;
        }
        self.receipt.as_ref().map(|receipt| HomeVolumeAttachment {
            request: self.request.clone(),
            receipt: receipt.clone(),
        })
    }
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.home")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_survives_restart_and_cannot_bind_replacement_storage() {
        let id = SandboxId("sandbox-home".into());
        let mut home = SandboxHome::reserve(
            &id,
            HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: "empty".into(),
                template_generation: "seed-generation".into(),
            },
        )
        .unwrap();
        let receipt = HomeVolumeReceipt {
            native_authority: None,
            volume_id: home.request.volume_id.clone(),
            request_sha256: home.request.computed_sha256(),
            storage_reference: "home-volume".into(),
            storage_generation: "original".into(),
        };
        home.bind(receipt.clone()).unwrap();
        let encoded = serde_json::to_vec(&home).unwrap();
        let mut reopened: SandboxHome = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(reopened, home);
        reopened.bind(receipt.clone()).unwrap();
        let mut replacement = receipt.clone();
        replacement.storage_generation = "reused-name".into();
        assert!(reopened.bind(replacement).is_err());
        assert!(reopened
            .validate(&SandboxId("other-sandbox".into()))
            .is_err());
        reopened.released = true;
        assert!(reopened.attachment().is_none());
        assert!(reopened.bind(receipt).is_err());
    }
}
