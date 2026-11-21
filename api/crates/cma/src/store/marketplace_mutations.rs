use layer2_core::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use layer2_core::profiles::store::ProfileStoreError;
use layer2_core::profiles::{
    AgentProfile, AgentProfileRef, ForkedProfile, MarketplaceError, MarketplaceKind,
    MarketplaceLocator, ProfileVisibility, SandboxProfile, SandboxProfileRef, WorkspaceProfile,
    WorkspaceProfileRef,
};
use mongodb::bson::{doc, Bson};

use super::marketplace_support::{
    collections_for_locator, find_public_profile, find_revision, fork_name, store_error,
    StoredProfile, StoredRevision,
};
use super::profile_draft_admission::{self, Error, Mutation, Pin, Pins};
use super::sandbox_mongo::{MongoProfileStore, DOCUMENT_SCHEMA_VERSION};

fn create_agent_fork(
    owner: &SubjectId,
    source: AgentProfile,
    revision: layer2_core::profiles::AgentProfileRevision,
    now: String,
) -> Result<ForkedProfile, MarketplaceError> {
    let profile = AgentProfile {
        internal: false,
        id: AgentProfileId::generate(),
        owner: owner.clone(),
        name: fork_name(&source.name),
        description: source.description,
        visibility: ProfileVisibility::Private,
        forked_from: Some(AgentProfileRef::new(source.id, revision.revision)),
        uses_count: 0,
        draft: revision.content,
        draft_version: 1,
        created_at: now.clone(),
        updated_at: now,
        retired_at: None,
    };
    profile
        .validate()
        .map_err(|error| store_error(error.into()))?;
    Ok(ForkedProfile::Agent(profile))
}

fn create_workspace_fork(
    owner: &SubjectId,
    source: WorkspaceProfile,
    revision: layer2_core::profiles::WorkspaceProfileRevision,
    now: String,
) -> Result<ForkedProfile, MarketplaceError> {
    let profile = WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId::generate(),
        owner: owner.clone(),
        name: fork_name(&source.name),
        description: source.description,
        visibility: ProfileVisibility::Private,
        forked_from: Some(WorkspaceProfileRef::new(source.id, revision.revision)),
        uses_count: 0,
        draft: revision.content,
        draft_version: 1,
        created_at: now.clone(),
        updated_at: now,
        retired_at: None,
    };
    profile
        .validate()
        .map_err(|error| store_error(error.into()))?;
    Ok(ForkedProfile::Workspace(profile))
}

fn create_sandbox_fork(
    owner: &SubjectId,
    source: SandboxProfile,
    revision: layer2_core::profiles::SandboxProfileRevision,
    now: String,
) -> ForkedProfile {
    let profile = SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: SandboxProfileId::generate(),
        owner: owner.clone(),
        name: fork_name(&source.name),
        description: source.description,
        visibility: ProfileVisibility::Private,
        forked_from: Some(SandboxProfileRef::new(source.id, revision.revision)),
        uses_count: 0,
        draft: revision.content,
        draft_version: 1,
        created_at: now.clone(),
        updated_at: now,
        retired_at: None,
    };
    ForkedProfile::Sandbox(profile)
}

pub(super) async fn fork_profile(
    store: &MongoProfileStore,
    owner: &SubjectId,
    locator: &MarketplaceLocator,
    revision: u32,
    now: String,
) -> Result<ForkedProfile, MarketplaceError> {
    let target = collections_for_locator(store, locator);
    let (_, source) = find_public_profile(target, locator.as_str()).await?;
    let revision = find_revision(target, locator.as_str(), revision).await?;
    let (pin, aggregates, revisions) = match target.kind {
        MarketplaceKind::Agent => (
            Pin::agent(locator.as_str(), revision.revision()),
            "agent_profiles",
            "agent_profile_revisions",
        ),
        MarketplaceKind::Workspace => (
            Pin::workspace(locator.as_str(), revision.revision()),
            "workspace_profiles",
            "workspace_profile_revisions",
        ),
        MarketplaceKind::Sandbox => (
            Pin::sandbox(locator.as_str(), revision.revision()),
            "sandbox_profiles",
            "sandbox_profile_revisions",
        ),
    };
    let mut pins = Pins::strict(pin);
    pins.guard(aggregates, doc! {
        "_id":locator.as_str(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "visibility":"public", "record.internal":{"$ne":true}, "retired_at":Bson::Null,
        "account_erasure_fenced":{"$ne":true},
        "record.id":locator.as_str(), "record.visibility":"public", "record.retired_at":Bson::Null,
    });
    pins.guard(
        revisions,
        doc! {
            "profile_id":locator.as_str(), "revision":i64::from(revision.revision()),
            "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        },
    );
    let fork = match (source, revision) {
        (StoredProfile::Agent(source), StoredRevision::Agent(revision)) => {
            create_agent_fork(owner, source, revision, now)
        }
        (StoredProfile::Workspace(source), StoredRevision::Workspace(revision)) => {
            create_workspace_fork(owner, source, revision, now)
        }
        (StoredProfile::Sandbox(source), StoredRevision::Sandbox(revision)) => {
            Ok(create_sandbox_fork(owner, source, revision, now))
        }
        _ => Err(MarketplaceError::Corrupt(
            "marketplace profile and revision kinds disagree".into(),
        )),
    }?;
    let document = match &fork {
        ForkedProfile::Agent(profile) => super::agent_profiles::document(profile),
        ForkedProfile::Workspace(profile) => super::workspace_profiles::document(profile),
        ForkedProfile::Sandbox(profile) => super::sandbox_profiles::document(profile),
    }
    .map_err(store_error)?;
    let db = store.client.database(&target.profiles.namespace().db);
    profile_draft_admission::transact(
        &store.client,
        &db,
        &mut pins,
        &Mutation::Insert {
            collection: target.profiles.clone(),
            document,
        },
    )
    .await
    .map_err(|error| match error {
        Error::ReferenceUnavailable => MarketplaceError::NotFound,
        Error::Driver(error) => MarketplaceError::Unavailable(error.to_string()),
    })?;
    Ok(fork)
}

fn saturating_increment() -> Bson {
    Bson::Document(doc! {
        "$cond": [
            { "$lt": ["$uses_count", i64::MAX] },
            { "$add": ["$uses_count", 1_i64] },
            "$uses_count",
        ]
    })
}

pub(super) async fn record_profile_use(
    store: &MongoProfileStore,
    locator: &MarketplaceLocator,
) -> Result<(), ProfileStoreError> {
    let target = collections_for_locator(store, locator);
    let result = target
        .profiles
        .update_one(
            doc! {
                "_id": locator.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "uses_count": { "$type": "long", "$gte": 0_i64 },
                "record.uses_count": { "$type": "long", "$gte": 0_i64 },
                "$expr": { "$eq": ["$uses_count", "$record.uses_count"] },
            },
            vec![doc! {
                "$set": {
                    "uses_count": saturating_increment(),
                    "record.uses_count": saturating_increment(),
                }
            }],
        )
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    if result.matched_count == 1 {
        return Ok(());
    }
    let Some(document) = target
        .profiles
        .find_one(doc! { "_id": locator.as_str() })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
    else {
        return Err(ProfileStoreError::NotFound);
    };
    validate_stored_profile(target.kind, &document)?;
    Err(ProfileStoreError::Corrupt(
        "profile use counters cannot be incremented atomically".into(),
    ))
}

fn validate_stored_profile(
    kind: layer2_core::profiles::MarketplaceKind,
    document: &mongodb::bson::Document,
) -> Result<(), ProfileStoreError> {
    let result = match kind {
        layer2_core::profiles::MarketplaceKind::Agent => {
            super::agent_profiles::read_profile(document).map(|_| ())
        }
        layer2_core::profiles::MarketplaceKind::Workspace => {
            super::workspace_profiles::read_profile(document).map(|_| ())
        }
        layer2_core::profiles::MarketplaceKind::Sandbox => {
            super::sandbox_profiles::read_profile(document).map(|_| ())
        }
    };
    result.map_err(|error| match error {
        ProfileStoreError::Corrupt(detail) => ProfileStoreError::Corrupt(detail),
        other => ProfileStoreError::Corrupt(other.to_string()),
    })
}
