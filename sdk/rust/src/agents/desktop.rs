#![doc = "Read desktop capability before formal Agent creation."]
use super::Agents;
use crate::Result;
use serde::{Deserialize, Serialize};

#[doc = "Existing creation context; omit both fields for a new environment."]
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct DesktopCreationScope {
    #[doc = "Owned Workspace to retain."]
    pub workspace_id: Option<String>,
    #[doc = "Owned Sandbox to retain."]
    pub sandbox_id: Option<String>,
}

string_enum! { #[doc = "Current capability, without allocating any resources."]
    pub enum DesktopAvailability {
        Available => "available", Unsupported => "unsupported",
        TemporarilyUnavailable => "temporarily_unavailable"
    }
}

#[doc = "Verified runtime requirements and safe capability reason."]
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DesktopCapability {
    #[doc = "Current availability."]
    pub state: DesktopAvailability,
    #[doc = "User-visible reason."]
    pub reason: String,
    #[doc = "Required runtime contract."]
    pub runtime_contract: u32,
    #[doc = "Minimum configured memory."]
    pub min_memory_mib: u32,
    #[doc = "Minimum configured CPU."]
    pub min_cpu_millis: u32,
    #[doc = "Verified immutable runtime image, if available."]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_image: Option<String>,
}

impl Agents<'_> {
    #[doc = "Read capability for the exact intended context; this creates no resources."]
    pub async fn desktop_capability(
        &self,
        scope: &DesktopCreationScope,
    ) -> Result<DesktopCapability> {
        let mut query = Vec::new();
        if let Some(value) = &scope.workspace_id {
            query.push(("workspace_id", value.clone()));
        }
        if let Some(value) = &scope.sandbox_id {
            query.push(("sandbox_id", value.clone()));
        }
        self.client.get(&["desktop-capability"], &query).await
    }
}
