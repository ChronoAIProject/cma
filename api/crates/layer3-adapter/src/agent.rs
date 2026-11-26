use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Immutable public profile material, fetched by the control plane. No credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillArchive {
    pub sha256: String,
    pub bytes: Vec<u8>,
}
use sha2::{Digest, Sha256};

impl SkillArchive {
    pub fn validate(&self) -> crate::Result<()> {
        if self.bytes.is_empty()
            || self.bytes.len() > 64 * 1024 * 1024
            || self.sha256 != format!("{:x}", Sha256::digest(&self.bytes))
        {
            return Err(crate::AdapterError::Rejected(
                "skill archive size or digest is invalid".into(),
            ));
        }
        Ok(())
    }
}

/// Stable slot identity. It is always the owning agent's identifier.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentSlot(pub String);

impl AgentSlot {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Model reasoning setting supported by the agent profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
}

impl ReasoningEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

/// Alias matching the profile-layer name without coupling the two layers.
pub type ThinkingEffort = ReasoningEffort;

/// Authority granted to an agent inside the outer sandbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    #[default]
    Guarded,
    Autonomous,
}

/// Runtime choices carried on every agent open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub reasoning_effort: ReasoningEffort,
    pub permission: PermissionLevel,
    pub own_credential: bool,
    /// Put the complete provider process behind an independent filesystem,
    /// process and network boundary. This requires the pinned runtime's bwrap.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolated_agent: bool,
    pub working_directory: String,
    pub home_directory: String,
    pub slot: AgentSlot,
}

/// One requested skill archive fetched by the runtime supervisor. The source
/// URL is a replaceable locator; the archive digest is the content identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkill {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub archive_sha256: String,
    pub source_url: String,
}

/// Agent-profile material to render into a provider-specific home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomeRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub options: RuntimeOptions,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
}

impl AgentHomeRequest {
    pub fn new(
        operation_id: impl Into<String>,
        options: RuntimeOptions,
        system_prompt: impl Into<String>,
        skills: Vec<AgentSkill>,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            options,
            system_prompt: system_prompt.into(),
            skills,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.agent.home.request.v1\0");
        options_field(&mut digest, &self.options);
        field(&mut digest, &self.system_prompt);
        skills_field(&mut digest, &self.skills);
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Open one agent after its home and worktree exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOpenRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub options: RuntimeOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_conversation: Option<String>,
    /// Transport authority for a numbered Open. Excluded from the launch digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_runtime: Option<crate::AgentRestartRuntime>,
}

impl AgentOpenRequest {
    pub fn new(
        operation_id: impl Into<String>,
        options: RuntimeOptions,
        resume_conversation: Option<String>,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            options,
            resume_conversation,
            restart_runtime: None,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.agent.open.request.v1\0");
        options_field(&mut digest, &self.options);
        optional_field(&mut digest, self.resume_conversation.as_deref());
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Stop one agent process while retaining its durable home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCloseRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
}

impl AgentCloseRequest {
    pub fn new(operation_id: impl Into<String>, agent_id: AgentSlot) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            agent_id,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        agent_mutation_sha256(b"chrono.agent.close.request.v1\0", &self.agent_id)
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Remove one stopped agent's durable home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomeRemoveRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
}

impl AgentHomeRemoveRequest {
    pub fn new(operation_id: impl Into<String>, agent_id: AgentSlot) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            agent_id,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        agent_mutation_sha256(b"chrono.agent.home.remove.request.v1\0", &self.agent_id)
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// One relative file materialized beneath an agent home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// One relative link materialized beneath an agent home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSymlink {
    pub path: String,
    pub target: String,
}

/// Provider-specific home plan transported without interpreting its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomePlan {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
    #[serde(default)]
    pub files: Vec<HomeFile>,
    #[serde(default)]
    pub symlinks: Vec<HomeSymlink>,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
}

/// Exact process launch for one agent slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLaunch {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
    pub cwd: String,
    pub home: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl AgentHomePlan {
    pub fn payload_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.agent.home.plan.v1\0");
        field(&mut digest, self.agent_id.as_str());
        count(&mut digest, self.files.len());
        for file in &self.files {
            field(&mut digest, &file.path);
            bytes_field(&mut digest, &file.bytes);
        }
        count(&mut digest, self.symlinks.len());
        for link in &self.symlinks {
            field(&mut digest, &link.path);
            field(&mut digest, &link.target);
        }
        skills_field(&mut digest, &self.skills);
        hex(&digest.finalize())
    }
}

impl AgentLaunch {
    pub fn payload_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.agent.launch.v1\0");
        field(&mut digest, self.agent_id.as_str());
        field(&mut digest, &self.cwd);
        field(&mut digest, &self.home);
        field(&mut digest, &self.program);
        count(&mut digest, self.args.len());
        for argument in &self.args {
            field(&mut digest, argument);
        }
        count(&mut digest, self.env.len());
        for (name, value) in &self.env {
            field(&mut digest, name);
            field(&mut digest, value);
        }
        hex(&digest.finalize())
    }
}

fn agent_mutation_sha256(prefix: &[u8], agent_id: &AgentSlot) -> String {
    let mut digest = Sha256::new();
    digest.update(prefix);
    field(&mut digest, agent_id.as_str());
    hex(&digest.finalize())
}

fn options_field(digest: &mut Sha256, options: &RuntimeOptions) {
    optional_field(digest, options.model.as_deref());
    field(digest, options.reasoning_effort.as_str());
    field(
        digest,
        match options.permission {
            PermissionLevel::Guarded => "guarded",
            PermissionLevel::Autonomous => "autonomous",
        },
    );
    digest.update([u8::from(options.own_credential)]);
    field(digest, &options.working_directory);
    field(digest, &options.home_directory);
    field(digest, options.slot.as_str());
    // Existing, non-isolated operations retain their durable request identity.
    if options.isolated_agent {
        field(digest, "isolated-agent-v1");
    }
}

fn skills_field(digest: &mut Sha256, skills: &[AgentSkill]) {
    count(digest, skills.len());
    for skill in skills {
        field(digest, &skill.id);
        optional_field(digest, skill.version.as_deref());
        field(digest, &skill.archive_sha256);
    }
    // Preserve existing unnamed intents while binding all newly resolved names.
    if skills.iter().any(|skill| skill.name.is_some()) {
        field(digest, "skill-names-v1");
        count(digest, skills.len());
        for skill in skills {
            optional_field(digest, skill.name.as_deref());
        }
    }
}

fn optional_field(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            digest.update([1]);
            field(digest, value);
        }
        None => digest.update([0]),
    }
}

fn bytes_field(digest: &mut Sha256, value: &[u8]) {
    count(digest, value.len());
    digest.update(value);
}

fn field(digest: &mut Sha256, value: &str) {
    bytes_field(digest, value.as_bytes());
}

fn count(digest: &mut Sha256, value: usize) {
    digest.update(u64::try_from(value).unwrap_or(u64::MAX).to_be_bytes());
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Opaque captured conversation archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSnapshot {
    pub conversation_id: String,
    pub archive_sha256: String,
    pub archive: Vec<u8>,
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
