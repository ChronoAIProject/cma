use layer2_core::profiles::store::ProfileStoreError;
use layer2_core::profiles::{
    AgentProfile, AgentProfileRevision, MarketplaceError, MarketplaceKind, MarketplaceLocator,
    MarketplaceRevision, MarketplaceSummary, ProfileVisibility, SandboxProfile,
    SandboxProfileRevision, WorkspaceProfile, WorkspaceProfileRevision,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::Collection;

use super::sandbox_mongo::{require_version, unsigned, MongoProfileStore};

pub(super) const SYSTEM_OWNER: &str = "system";

#[derive(Clone, Copy)]
pub(super) struct ProfileCollections<'a> {
    pub kind: MarketplaceKind,
    pub profiles: &'a Collection<Document>,
    pub revisions: &'a Collection<Document>,
}

pub(super) fn collections_for_kind(
    store: &MongoProfileStore,
    kind: MarketplaceKind,
) -> ProfileCollections<'_> {
    match kind {
        MarketplaceKind::Agent => ProfileCollections {
            kind,
            profiles: &store.agent_profiles,
            revisions: &store.agent_revisions,
        },
        MarketplaceKind::Workspace => ProfileCollections {
            kind,
            profiles: &store.workspace_profiles,
            revisions: &store.workspace_revisions,
        },
        MarketplaceKind::Sandbox => ProfileCollections {
            kind,
            profiles: &store.sandbox_profiles,
            revisions: &store.sandbox_revisions,
        },
    }
}

pub(super) fn collections_for_locator<'a>(
    store: &'a MongoProfileStore,
    locator: &MarketplaceLocator,
) -> ProfileCollections<'a> {
    collections_for_kind(store, locator.kind())
}

pub(super) enum StoredProfile {
    Agent(AgentProfile),
    Workspace(WorkspaceProfile),
    Sandbox(SandboxProfile),
}

impl StoredProfile {
    pub fn id(&self) -> &str {
        match self {
            Self::Agent(profile) => profile.id.as_str(),
            Self::Workspace(profile) => profile.id.as_str(),
            Self::Sandbox(profile) => profile.id.as_str(),
        }
    }

    pub fn owner(&self) -> &layer2_core::ids::SubjectId {
        match self {
            Self::Agent(profile) => &profile.owner,
            Self::Workspace(profile) => &profile.owner,
            Self::Sandbox(profile) => &profile.owner,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Agent(profile) => &profile.name,
            Self::Workspace(profile) => &profile.name,
            Self::Sandbox(profile) => &profile.name,
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Self::Agent(profile) => &profile.description,
            Self::Workspace(profile) => &profile.description,
            Self::Sandbox(profile) => &profile.description,
        }
    }

    pub fn internal(&self) -> bool {
        match self {
            Self::Agent(profile) => profile.internal,
            Self::Workspace(profile) => profile.internal,
            Self::Sandbox(profile) => profile.internal,
        }
    }

    pub fn visibility(&self) -> ProfileVisibility {
        match self {
            Self::Agent(profile) => profile.visibility,
            Self::Workspace(profile) => profile.visibility,
            Self::Sandbox(profile) => profile.visibility,
        }
    }

    pub fn retired_at(&self) -> Option<&String> {
        match self {
            Self::Agent(profile) => profile.retired_at.as_ref(),
            Self::Workspace(profile) => profile.retired_at.as_ref(),
            Self::Sandbox(profile) => profile.retired_at.as_ref(),
        }
    }

    pub fn uses_count(&self) -> u64 {
        match self {
            Self::Agent(profile) => profile.uses_count,
            Self::Workspace(profile) => profile.uses_count,
            Self::Sandbox(profile) => profile.uses_count,
        }
    }
}

pub(super) enum StoredRevision {
    Agent(AgentProfileRevision),
    Workspace(WorkspaceProfileRevision),
    Sandbox(SandboxProfileRevision),
}

impl StoredRevision {
    pub fn revision(&self) -> u32 {
        match self {
            Self::Agent(revision) => revision.revision,
            Self::Workspace(revision) => revision.revision,
            Self::Sandbox(revision) => revision.revision,
        }
    }

    pub fn published_at(&self) -> &str {
        match self {
            Self::Agent(revision) => &revision.published_at,
            Self::Workspace(revision) => &revision.published_at,
            Self::Sandbox(revision) => &revision.published_at,
        }
    }

    pub fn into_marketplace(self) -> MarketplaceRevision {
        match self {
            Self::Agent(revision) => MarketplaceRevision::Agent(revision),
            Self::Workspace(revision) => MarketplaceRevision::Workspace(revision),
            Self::Sandbox(revision) => MarketplaceRevision::Sandbox(revision),
        }
    }
}

pub(super) fn decode_profile(
    kind: MarketplaceKind,
    document: &Document,
) -> Result<StoredProfile, MarketplaceError> {
    let profile = match kind {
        MarketplaceKind::Agent => StoredProfile::Agent(
            super::agent_profiles::read_profile(document).map_err(store_error)?,
        ),
        MarketplaceKind::Workspace => StoredProfile::Workspace(
            super::workspace_profiles::read_profile(document).map_err(store_error)?,
        ),
        MarketplaceKind::Sandbox => StoredProfile::Sandbox(
            super::sandbox_profiles::read_profile(document).map_err(store_error)?,
        ),
    };
    Ok(profile)
}

pub(super) fn decode_revision(
    kind: MarketplaceKind,
    document: &Document,
) -> Result<StoredRevision, MarketplaceError> {
    let revision = match kind {
        MarketplaceKind::Agent => StoredRevision::Agent(
            super::agent_profiles::read_revision(document).map_err(store_error)?,
        ),
        MarketplaceKind::Workspace => StoredRevision::Workspace(
            super::workspace_profiles::read_revision(document).map_err(store_error)?,
        ),
        MarketplaceKind::Sandbox => StoredRevision::Sandbox(
            super::sandbox_profiles::read_revision(document).map_err(store_error)?,
        ),
    };
    Ok(revision)
}

#[derive(Clone)]
pub(super) struct PublicationMetadata {
    pub revision: u32,
    pub published_at: Option<String>,
}

pub(super) fn publication_metadata(
    document: &Document,
) -> Result<PublicationMetadata, MarketplaceError> {
    require_version(document).map_err(MarketplaceError::Corrupt)?;
    let revision = document
        .get_i64("last_revision")
        .map_err(|error| MarketplaceError::Corrupt(error.to_string()))
        .and_then(|value| {
            unsigned(value, "last profile revision").map_err(MarketplaceError::Corrupt)
        })
        .and_then(|value| {
            u32::try_from(value)
                .map_err(|_| MarketplaceError::Corrupt("last profile revision is too large".into()))
        })?;
    let published_at = match document.get("latest_published_at") {
        Some(Bson::String(value)) => Some(value.clone()),
        Some(Bson::Null) => None,
        Some(value) => {
            return Err(MarketplaceError::Corrupt(format!(
                "latest publication time has unexpected value {value}"
            )))
        }
        None => {
            return Err(MarketplaceError::Corrupt(
                "profile has no latest publication time".into(),
            ))
        }
    };
    if (revision == 0) != published_at.is_none() {
        return Err(MarketplaceError::Corrupt(
            "profile revision and publication time disagree".into(),
        ));
    }
    Ok(PublicationMetadata {
        revision,
        published_at,
    })
}

pub(super) fn marketplace_summary(
    kind: MarketplaceKind,
    document: &Document,
    authors: &std::collections::BTreeMap<String, String>,
) -> Result<MarketplaceSummary, MarketplaceError> {
    let profile = decode_profile(kind, document)?;
    if profile.internal()
        || profile.visibility() != ProfileVisibility::Public
        || profile.retired_at().is_some()
    {
        return Err(MarketplaceError::Corrupt(
            "marketplace query returned a hidden profile".into(),
        ));
    }
    let publication = publication_metadata(document)?;
    let published_at = publication.published_at.ok_or_else(|| {
        MarketplaceError::Corrupt("public profile has no published revision".into())
    })?;
    Ok(MarketplaceSummary {
        kind,
        id: profile.id().to_string(),
        name: profile.name().to_string(),
        description: profile.description().to_string(),
        owner_display: authors
            .get(profile.owner().as_str())
            .cloned()
            .unwrap_or_else(|| {
                if profile.owner().as_str() == super::default_profiles::SYSTEM_OWNER {
                    "CMA".into()
                } else {
                    "Author unavailable".into()
                }
            }),
        latest_revision: publication.revision,
        published_at,
        uses_count: profile.uses_count(),
    })
}

pub(super) async fn find_public_profile(
    target: ProfileCollections<'_>,
    id: &str,
) -> Result<(Document, StoredProfile), MarketplaceError> {
    let document = target
        .profiles
        .find_one(doc! {
            "_id": id,
            "visibility": "public",
            "record.internal": { "$ne": true },
            "retired_at": Bson::Null,
        })
        .await
        .map_err(driver_error)?
        .ok_or(MarketplaceError::NotFound)?;
    let profile = decode_profile(target.kind, &document)?;
    Ok((document, profile))
}

pub(super) async fn find_revision(
    target: ProfileCollections<'_>,
    id: &str,
    revision: u32,
) -> Result<StoredRevision, MarketplaceError> {
    let document = target
        .revisions
        .find_one(doc! {
            "profile_id": id,
            "revision": i64::from(revision),
        })
        .await
        .map_err(driver_error)?
        .ok_or(MarketplaceError::NotFound)?;
    decode_revision(target.kind, &document)
}

pub(super) fn fork_name(source: &str) -> String {
    let mut name = String::new();
    for character in source.chars() {
        if name.len() + character.len_utf8() > 75 {
            break;
        }
        name.push(character);
    }
    name.push_str(" copy");
    name
}

pub(super) fn store_error(error: ProfileStoreError) -> MarketplaceError {
    match error {
        ProfileStoreError::NotFound => MarketplaceError::NotFound,
        ProfileStoreError::Validation(error) => MarketplaceError::Validation(error.code.into()),
        ProfileStoreError::Unavailable(message) => MarketplaceError::Unavailable(message),
        ProfileStoreError::Corrupt(message) => MarketplaceError::Corrupt(message),
        ProfileStoreError::Duplicate => {
            MarketplaceError::Unavailable("generated profile identifier already exists".into())
        }
        ProfileStoreError::Conflict => {
            MarketplaceError::Unavailable("profile changed during marketplace operation".into())
        }
        ProfileStoreError::ReferenceUnavailable => {
            MarketplaceError::Corrupt("published profile reference is unavailable".into())
        }
        ProfileStoreError::Protected => MarketplaceError::Forbidden,
    }
}

pub(super) fn driver_error(error: mongodb::error::Error) -> MarketplaceError {
    MarketplaceError::Unavailable(error.to_string())
}
