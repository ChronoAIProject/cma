use layer2_core::profiles::authoring::{
    AuthoringServiceEntry, AuthoringSkillEntry, RepositoryResolution,
};
use layer2_core::profiles::ProfileCapacityBounds;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub(in super::super) struct ProfileCapacityBoundsView {
    min_cpu_millis: u32,
    max_cpu_millis: u32,
    min_memory_mib: u32,
    max_memory_mib: u32,
}

impl From<ProfileCapacityBounds> for ProfileCapacityBoundsView {
    fn from(value: ProfileCapacityBounds) -> Self {
        Self {
            min_cpu_millis: value.min_cpu_millis,
            max_cpu_millis: value.max_cpu_millis,
            min_memory_mib: value.min_memory_mib,
            max_memory_mib: value.max_memory_mib,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(in super::super) struct AuthoringServiceView {
    id: String,
    /// Canonical catalog identity, distinct from a selectable service alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    catalog_id: Option<String>,
    /// Browser-facing catalog URL in the identity environment serving this CMA.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    detail_url: Option<String>,
    name: String,
    /// Complete source description, at most 6144 UTF-8 bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    description: Option<String>,
}

impl AuthoringServiceView {
    pub(super) fn new(value: AuthoringServiceEntry, public_origin: Option<&url::Url>) -> Self {
        let detail_url = value
            .catalog_id
            .as_deref()
            .zip(public_origin)
            .and_then(|(id, origin)| {
                if matches!(id, "" | "." | "..") {
                    return None;
                }
                let mut url = origin.clone();
                url.path_segments_mut()
                    .ok()?
                    .pop_if_empty()
                    .extend(["services", id]);
                Some(url.to_string())
            });
        Self {
            id: value.id.to_string(),
            catalog_id: value.catalog_id,
            detail_url,
            name: value.name,
            description: value.description,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(in super::super) struct AuthoringSkillView {
    id: String,
    name: String,
    /// Complete source description, at most 6144 UTF-8 bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    description: Option<String>,
    /// Up to 100 latest selectable releases, newest-first by numeric major.minor.
    #[schema(min_items = 1, max_items = 100)]
    versions: Vec<String>,
}

impl From<AuthoringSkillEntry> for AuthoringSkillView {
    fn from(value: AuthoringSkillEntry) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name,
            description: value.description,
            versions: value.versions,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(in super::super) struct AuthoringSkillPage {
    pub items: Vec<AuthoringSkillView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub next: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub(in super::super) struct SkillCatalogParameters {
    #[param(max_length = 200)]
    pub q: Option<String>,
    #[param(minimum = 1, maximum = 50, default = 20)]
    pub limit: Option<u16>,
    #[param(max_length = 2048)]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct ResolveRepositoryRequest {
    pub url: String,
    pub reference: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(in super::super) struct RepositoryResolutionView {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    reference: Option<String>,
    default_branch: String,
    commit_sha: String,
}

impl From<RepositoryResolution> for RepositoryResolutionView {
    fn from(value: RepositoryResolution) -> Self {
        Self {
            url: value.url,
            reference: value.reference,
            default_branch: value.default_branch,
            commit_sha: value.commit_sha,
        }
    }
}
