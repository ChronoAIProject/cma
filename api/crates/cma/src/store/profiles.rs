//! Shared document shape, indexes, and paging for schema-v1 profiles.

use crate::store::transaction_terminal::GuardedTerminal as _;
pub(super) mod summaries;

use futures_util::TryStreamExt;
use layer2_core::ids::SubjectId;
use layer2_core::profiles::store::{ProfilePage, ProfilePageRequest, ProfileStoreError};
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::{IndexOptions, ReadConcern, ReturnDocument};
use mongodb::{ClientSession, Collection, IndexModel};
use serde::de::DeserializeOwned;

use super::sandbox_mongo::{
    require_version, signed, unsigned, MongoProfileStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) const MAX_PROFILE_PAGE: u16 = 200;

pub(super) fn profile_indexes(prefix: &str) -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "owner": 1, "updated_at": -1 })
            .options(
                IndexOptions::builder()
                    .name(format!("{prefix}_owner_updated"))
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "visibility": 1, "kind": 1, "updated_at": -1 })
            .options(
                IndexOptions::builder()
                    .name(format!("{prefix}_marketplace"))
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! {
                "visibility": 1,
                "retired_at": 1,
                "latest_published_at": -1,
                "name": 1,
                "_id": 1,
            })
            .options(
                IndexOptions::builder()
                    .name(format!("{prefix}_marketplace_order"))
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "name": "text", "description": "text" })
            .options(
                IndexOptions::builder()
                    .name(format!("{prefix}_marketplace_text"))
                    .build(),
            )
            .build(),
    ]
}

pub(super) fn revision_indexes(prefix: &str) -> Vec<IndexModel> {
    vec![IndexModel::builder()
        .keys(doc! { "profile_id": 1, "revision": 1 })
        .options(
            IndexOptions::builder()
                .name(format!("{prefix}_profile_revision"))
                .unique(true)
                .build(),
        )
        .build()]
}

pub(super) async fn ensure_indexes(
    store: &MongoProfileStore,
) -> Result<(), layer2_core::StoreError> {
    let collections = [
        (&store.agent_profiles, profile_indexes("agent_profiles")),
        (
            &store.agent_revisions,
            revision_indexes("agent_profile_revisions"),
        ),
        (
            &store.workspace_profiles,
            profile_indexes("workspace_profiles"),
        ),
        (
            &store.workspace_revisions,
            revision_indexes("workspace_profile_revisions"),
        ),
        (&store.sandbox_profiles, profile_indexes("sandbox_profiles")),
        (
            &store.sandbox_revisions,
            revision_indexes("sandbox_profile_revisions"),
        ),
    ];
    for (collection, indexes) in collections {
        collection.create_indexes(indexes).await.map_err(|error| {
            layer2_core::StoreError::Unavailable(format!(
                "profile indexes on {}: {error}",
                collection.name()
            ))
        })?;
    }
    Ok(())
}

pub(super) async fn retire_profile(
    collection: &Collection<Document>,
    id: &str,
    owner: &SubjectId,
    retired_at: String,
) -> Result<(), ProfileStoreError> {
    let mut filter = doc! {
        "_id": id,
        "owner": owner.as_str(),
        "record.internal": { "$ne": true },
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "retired_at": Bson::Null,
    };
    let result = collection
        .update_one(
            filter.clone(),
            doc! {
                "$set": {
                    "updated_at": &retired_at,
                    "retired_at": &retired_at,
                    "record.updated_at": &retired_at,
                    "record.retired_at": &retired_at,
                }
            },
        )
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    if result.matched_count == 1 {
        return Ok(());
    }
    filter.insert("retired_at", doc! { "$ne": Bson::Null });
    collection
        .find_one(filter)
        .projection(doc! { "_id": 1 })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
        .map(|_| ())
        .ok_or(ProfileStoreError::NotFound)
}

fn validate_page(page: &ProfilePageRequest) -> Result<(), ProfileStoreError> {
    if page.limit == 0 || page.limit > MAX_PROFILE_PAGE {
        Err(layer2_core::profiles::ProfileError::new("invalid_page", "limit").into())
    } else {
        Ok(())
    }
}

pub(super) async fn list_profiles<T: DeserializeOwned>(
    collection: &Collection<Document>,
    owner: Option<&SubjectId>,
    public_only: bool,
    page: &ProfilePageRequest,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<ProfilePage<T>, ProfileStoreError> {
    validate_page(page)?;
    let mut filter = doc! { "record.internal": { "$ne": true } };
    if let Some(owner) = owner {
        filter.insert("owner", owner.as_str());
    }
    if public_only {
        filter.insert("visibility", "public");
        filter.insert("retired_at", Bson::Null);
    }
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    let mut cursor = collection
        .find(filter)
        .sort(doc! { "_id": 1 })
        .limit(i64::from(page.limit) + 1)
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    let mut rows = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
    {
        let id = document
            .get_str("_id")
            .map_err(|error| ProfileStoreError::Corrupt(error.to_string()))?
            .to_string();
        rows.push((id, decode(&document)?));
    }
    let has_more = rows.len() > usize::from(page.limit);
    if has_more {
        rows.pop();
    }
    let next = has_more.then(|| {
        rows.last()
            .expect("a bounded page has a last row")
            .0
            .clone()
    });
    Ok(ProfilePage {
        items: rows.into_iter().map(|(_, record)| record).collect(),
        next,
    })
}

pub(super) async fn get_profile<T: DeserializeOwned>(
    collection: &Collection<Document>,
    id: &str,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<Option<T>, ProfileStoreError> {
    collection
        .find_one(doc! { "_id": id })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(decode)
        .transpose()
}

pub(super) async fn get_revision<T: DeserializeOwned>(
    collection: &Collection<Document>,
    profile_id: &str,
    revision: u32,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<Option<T>, ProfileStoreError> {
    collection
        .find_one(doc! {
            "profile_id": profile_id,
            "revision": i64::from(revision),
        })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(decode)
        .transpose()
}

/// Paces a retry of initial publication after a confirmed-uncommitted
/// conflict. The competing initial publisher holds the same unpublished row
/// until it commits; the pause lets that commit land, so the retry reconciles
/// its revision one instead of exhausting the budget. Ordinary publication
/// keeps yielding.
pub(super) async fn retry_pause(attempt: usize, initial: bool) {
    if attempt == 0 {
        return;
    }
    if initial {
        let shift = u32::try_from(attempt - 1).unwrap_or(u32::MAX).min(4);
        tokio::time::sleep(std::time::Duration::from_millis(10 << shift)).await;
    } else {
        tokio::task::yield_now().await;
    }
}

/// Bounds the wait for an unknown commit to become majority-visible.
const UNKNOWN_COMMIT_READ_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Reads one exact row after a commit whose reply was lost. A read-only
/// successor transaction in the same causally consistent session retires any
/// open server state. Its majority read waits, within a bound, for the
/// session's commit time, so a row it returns is durable; a read outside the
/// session may not yet see a commit that applied.
pub(super) async fn read_after_unknown_commit(
    session: &mut ClientSession,
    collection: &Collection<Document>,
    filter: Document,
) -> Result<Option<Document>, ProfileStoreError> {
    session
        .start_transaction()
        .read_concern(ReadConcern::majority())
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    let row = collection
        .find_one(filter)
        .max_time(UNKNOWN_COMMIT_READ_LIMIT)
        .session(&mut *session)
        .await;
    let _ = session.abort_guarded().await;
    row.map_err(|error| ProfileStoreError::Unavailable(error.to_string()))
}

pub(super) async fn get_revision_after_unknown_commit<T: DeserializeOwned>(
    session: &mut ClientSession,
    collection: &Collection<Document>,
    profile_id: &str,
    revision: u32,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<Option<T>, ProfileStoreError> {
    read_after_unknown_commit(
        session,
        collection,
        doc! { "profile_id": profile_id, "revision": i64::from(revision) },
    )
    .await?
    .as_ref()
    .map(decode)
    .transpose()
}

pub(super) async fn list_revisions<T: DeserializeOwned>(
    profiles: &Collection<Document>,
    revisions: &Collection<Document>,
    profile_id: &str,
    page: &ProfilePageRequest,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<ProfilePage<T>, ProfileStoreError> {
    validate_page(page)?;
    if profiles
        .find_one(doc! { "_id": profile_id })
        .projection(doc! { "document_schema_version": 1 })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
        .is_none()
    {
        return Err(ProfileStoreError::NotFound);
    }
    let mut filter = doc! { "profile_id": profile_id };
    if let Some(cursor) = &page.cursor {
        let revision = cursor
            .parse::<u32>()
            .map_err(|_| layer2_core::profiles::ProfileError::new("invalid_page", "cursor"))?;
        filter.insert("revision", doc! { "$lt": i64::from(revision) });
    }
    let mut cursor = revisions
        .find(filter)
        .sort(doc! { "revision": -1 })
        .limit(i64::from(page.limit) + 1)
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    let mut rows = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
    {
        let revision = document
            .get_i64("revision")
            .map_err(|error| ProfileStoreError::Corrupt(error.to_string()))
            .and_then(|value| {
                unsigned(value, "profile revision").map_err(ProfileStoreError::Corrupt)
            })?;
        rows.push((revision, decode(&document)?));
    }
    let has_more = rows.len() > usize::from(page.limit);
    if has_more {
        rows.pop();
    }
    let next = has_more.then(|| {
        rows.last()
            .expect("a bounded page has a last row")
            .0
            .to_string()
    });
    Ok(ProfilePage {
        items: rows.into_iter().map(|(_, record)| record).collect(),
        next,
    })
}

pub(super) enum PublishError {
    Domain(ProfileStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn advance_publication(
    collection: &Collection<Document>,
    session: &mut ClientSession,
    id: &str,
    expected: u64,
    published_at: &str,
    initial_only: bool,
) -> Result<(Document, u32), PublishError> {
    let mut filter = doc! {
        "_id": id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "draft_version": signed(expected, "expected profile draft version")
            .map_err(ProfileStoreError::Corrupt)
            .map_err(PublishError::Domain)?,
        "retired_at": Bson::Null,
        "account_erasure_fenced": { "$ne": true },
    };
    if initial_only {
        // Same draft version can be published repeatedly by the ordinary API.
        // Initial creation must consume the unpublished state in this write.
        filter.insert("last_revision", 0_i64);
    }
    let profile = collection
        .find_one_and_update(
            filter,
            doc! {
                "$inc": { "last_revision": 1_i64 },
                "$set": { "latest_published_at": published_at },
            },
        )
        .return_document(ReturnDocument::After)
        .session(&mut *session)
        .await
        .map_err(PublishError::Driver)?;
    let Some(profile) = profile else {
        let exists = collection
            .find_one(doc! { "_id": id })
            .projection(doc! { "document_schema_version": 1 })
            .session(session)
            .await
            .map_err(PublishError::Driver)?;
        return Err(PublishError::Domain(if exists.is_some() {
            ProfileStoreError::Conflict
        } else {
            ProfileStoreError::NotFound
        }));
    };
    require_version(&profile)
        .map_err(ProfileStoreError::Corrupt)
        .map_err(PublishError::Domain)?;
    let number = profile
        .get_i64("last_revision")
        .map_err(|error| ProfileStoreError::Corrupt(error.to_string()))
        .and_then(|value| unsigned(value, "profile revision").map_err(ProfileStoreError::Corrupt))
        .and_then(|value| u32::try_from(value).map_err(|_| ProfileStoreError::Conflict))
        .map_err(PublishError::Domain)?;
    Ok((profile, number))
}
