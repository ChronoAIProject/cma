//! Actual public projections; persistence-only identities never enter schemas.
use crate::{model::*, permissions::Role};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct ImageView {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub shares: Vec<String>,
    pub visibility: VisibleImageScope,
    pub retired: bool,
    pub revision: i64,
    pub created_at_ms: i64,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VisibleImageScope {
    Private,
    Shared,
    Public,
}
impl ImageView {
    pub(crate) fn new(image: Image, actor: &str) -> Self {
        let visibility = if image.visibility == ImageVisibility::Public {
            VisibleImageScope::Public
        } else if image.owner == actor {
            VisibleImageScope::Private
        } else {
            VisibleImageScope::Shared
        };
        let shares = if image.owner == actor {
            image.shares
        } else {
            vec![]
        };
        Self {
            id: image.id,
            owner: image.owner,
            name: image.spec.name,
            description: image.spec.description,
            shares,
            visibility,
            retired: image.retired,
            revision: image.revision,
            created_at_ms: image.created_at_ms,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct BuildView {
    pub id: String,
    pub image_id: String,
    pub source: BuildInput,
    pub phase: BuildPhase,
    pub error: Option<String>,
    pub revision: i64,
    pub reference: Option<String>,
    pub digest: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
impl BuildView {
    pub(crate) fn new(build: Build, registry: &ImageRegistry) -> Self {
        let reference = registry.reference(&build);
        Self {
            id: build.id,
            image_id: build.image_id,
            source: build.source,
            phase: build.phase,
            error: build.error,
            revision: build.revision,
            reference,
            digest: build.digest,
            created_at_ms: build.created_at_ms,
            updated_at_ms: build.updated_at_ms,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct VersionView {
    pub id: String,
    pub image_id: String,
    pub phase: BuildPhase,
    pub reference: Option<String>,
    pub digest: Option<String>,
    pub created_at_ms: i64,
}
impl VersionView {
    pub(crate) fn new(build: Build, registry: &ImageRegistry) -> Self {
        let reference = registry.reference(&build);
        Self {
            id: build.id,
            image_id: build.image_id,
            phase: build.phase,
            reference,
            digest: build.digest,
            created_at_ms: build.created_at_ms,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct PageView<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct PermissionsView {
    pub subject: String,
    pub permissions: Vec<String>,
    pub roles: &'static [Role],
}
#[derive(Serialize, ToSchema)]
pub(crate) struct Problem {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: Option<&'static str>,
    pub status: u16,
    pub detail: String,
}
