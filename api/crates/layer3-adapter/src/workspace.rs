use serde::{Deserialize, Serialize};

use crate::CommandReceipt;

/// How a workspace root is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceSource {
    Clone {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Init,
}

/// Prepare one named directory directly below the sandbox home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRequest {
    pub name: String,
    pub source: WorkspaceSource,
}

/// The canonical workspace facts resolved by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceReceipt {
    pub name: String,
    pub path: String,
    pub resolved_commit: String,
}

/// Run workspace-profile setup commands in its main checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSetupRequest {
    pub name: String,
    #[serde(default)]
    pub commands: Vec<String>,
}

/// Proof that every setup command completed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupReceipt {
    #[serde(default)]
    pub commands: Vec<CommandReceipt>,
}

/// Add one independently writable Git worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRequest {
    pub workspace: String,
    pub name: String,
    pub base: String,
}

/// Stable reference used to remove a worktree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRef {
    pub workspace: String,
    pub name: String,
}

/// Paths and branch resolved by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeReceipt {
    pub workspace: String,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
}
