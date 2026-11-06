//! Bounded external profile input, separate from retained environment snapshots.
use crate::profiles::{EnvironmentVariable, SkillRef, WorkspaceLocation};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExternalProfile {
    pub schema_version: u16,
    #[serde(default)]
    pub queue_lifetime_seconds: Option<u32>,
    pub metadata: Metadata,
    pub model: Model,
    pub permission: Permission,
    pub context: Context,
    pub environment: Option<Environment>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Metadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub owner: Owner,
    pub provider: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Owner {
    pub subject: String,
    pub kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Model {
    pub access: String,
    pub service: String,
    pub api_type: String,
    pub model: String,
    pub thinking_effort: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Permission {
    pub action_mode: String,
    #[serde(default)]
    pub service_scope: Vec<String>,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct Context {
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    #[serde(rename = "mcp_servers")]
    pub external_tools: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Environment {
    #[serde(default)]
    pub isolated_agents: bool,
    #[serde(default)]
    pub session_affinity: super::profile::SessionAffinity,
    #[serde(default)]
    pub always_keep_alive: bool,
    pub size: Size,
    pub workspace: WorkspaceLocation,
    #[serde(default)]
    pub setup_commands: Vec<Setup>,
    #[serde(default)]
    pub environment_variables: Vec<EnvironmentVariable>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Size {
    pub cpu_cores: f64,
    pub memory_gib: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Setup {
    pub phase: String,
    pub command: String,
}
