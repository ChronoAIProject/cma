//! Public deployment-selected profile defaults.

use crate::{Client, Result, profiles::ProfileReference};
use serde::{Deserialize, Serialize};

/// Current platform profile choices without private operator settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformDefaults {
    /// Default Sandbox profile revision.
    pub sandbox_profile: ProfileReference,
    /// Default Workspace profile revision.
    pub workspace_profile: ProfileReference,
    /// Default Agent profile revision.
    pub agent_profile: ProfileReference,
}

impl Client {
    /// Read the current deployment profile defaults.
    pub async fn read_defaults(&self) -> Result<PlatformDefaults> {
        self.get(&["defaults"], &[]).await
    }
}
