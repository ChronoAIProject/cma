//! Typed read-only profile authoring metadata.

use serde::{Deserialize, Deserializer, Serialize};

fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// Immutable deployment capacity policy available to profile authors.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileAuthoringBounds {
    /// Minimum Sandbox CPU allocation in millicores.
    pub min_cpu_millis: u32,
    /// Maximum Sandbox CPU allocation in millicores.
    pub max_cpu_millis: u32,
    /// Minimum Sandbox memory allocation in MiB.
    pub min_memory_mib: u32,
    /// Maximum Sandbox memory allocation in MiB.
    pub max_memory_mib: u32,
}

/// Safe service catalog metadata without connection credentials.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileAuthoringService {
    /// Service identity for a Sandbox profile's scope list.
    pub id: String,
    /// Canonical service catalog identity, distinct from a selectable alias.
    #[serde(default, deserialize_with = "present")]
    pub catalog_id: Option<String>,
    /// Service display name.
    pub name: String,
    /// Optional public description.
    #[serde(default, deserialize_with = "present")]
    pub description: Option<String>,
}

/// Published skill choices, retaining exact version strings and server order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileAuthoringSkill {
    /// Registry-local skill identity.
    pub id: String,
    /// Skill display name.
    pub name: String,
    /// Optional public description.
    #[serde(default, deserialize_with = "present")]
    pub description: Option<String>,
    /// Canonical literal `major.minor` versions, newest first.
    pub versions: Vec<String>,
}

/// One bounded skill page with an opaque continuation cursor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileAuthoringSkillPage {
    /// Selectable skills on this page; filtering can leave an empty page before continuation.
    pub items: Vec<ProfileAuthoringSkill>,
    /// Opaque continuation cursor, omitted at the end of the catalog.
    #[serde(default, deserialize_with = "present")]
    pub next: Option<String>,
}

/// Optional skill search inputs; omitted values use the server defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileAuthoringSkillSearch {
    /// Search text, at most 200 UTF-8 bytes without control characters.
    pub q: Option<String>,
    /// Requested page size from 1 through 50; defaults to 20.
    pub limit: Option<u16>,
    /// Nonempty server-issued cursor, at most 2048 UTF-8 bytes without controls.
    pub cursor: Option<String>,
}

/// Read-only verification of a public GitHub repository and optional reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveRepositoryRequest {
    /// Public GitHub repository URL; authoritative validation occurs on the server.
    pub url: String,
    /// Optional branch, tag, or commit; omission retains default-branch semantics.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub reference: Option<String>,
}

/// Authoritative repository identity and verified commit without cloning it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResolvedRepository {
    /// Canonical public GitHub URL observed by the server.
    pub url: String,
    /// Exact requested reference, absent when none was supplied.
    #[serde(default, deserialize_with = "present")]
    pub reference: Option<String>,
    /// Current repository default branch.
    pub default_branch: String,
    /// Authoritative lowercase 40-digit hexadecimal commit identity.
    pub commit_sha: String,
}
