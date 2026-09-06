use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentProfileId, SandboxProfileId, WorkspaceProfileId};

pub const MAX_PROFILE_NAME_BYTES: usize = 80;
pub const MAX_DESCRIPTION_BYTES: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileVisibility {
    Private,
    Public,
}

impl ProfileVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRef<I> {
    pub id: I,
    pub revision: u32,
}

impl<I> ProfileRef<I> {
    pub fn new(id: I, revision: u32) -> Self {
        Self { id, revision }
    }
}

pub type AgentProfileRef = ProfileRef<AgentProfileId>;
pub type WorkspaceProfileRef = ProfileRef<WorkspaceProfileId>;
pub type SandboxProfileRef = ProfileRef<SandboxProfileId>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {field}")]
pub struct ProfileError {
    pub code: &'static str,
    pub field: Cow<'static, str>,
}

impl ProfileError {
    pub const fn new(code: &'static str, field: &'static str) -> Self {
        Self {
            code,
            field: Cow::Borrowed(field),
        }
    }

    pub fn environment_secret(name: &str) -> Self {
        Self {
            code: "secret_like_environment_value",
            field: Cow::Owned(format!("environment_variables.{name}")),
        }
    }
}

pub fn validate_metadata(name: &str, description: &str) -> Result<(), ProfileError> {
    if name.trim().is_empty() || name.len() > MAX_PROFILE_NAME_BYTES {
        return Err(ProfileError::new("invalid_name", "name"));
    }
    if description.len() > MAX_DESCRIPTION_BYTES {
        return Err(ProfileError::new("description_too_long", "description"));
    }
    Ok(())
}
