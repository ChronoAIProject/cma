//! Shared document shape, indexes, and paging for schema-v1 profiles.

use futures_util::TryStreamExt;
use layer2_core::ids::SubjectId;
use layer2_core::profiles::store::{ProfilePage, ProfilePageRequest, ProfileStoreError};
use layer2_core::profiles::ProfileVisibility;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::{IndexOptions, ReturnDocument};
use mongodb::{ClientSession, Collection, IndexModel};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::sandbox_mongo::{
    is_duplicate_key, nullable, read_record, record_bson, require_version, signed, unsigned,
    MongoProfileStore, DOCUMENT_SCHEMA_VERSION,
};

const MAX_PROFILE_PAGE: u16 = 200;

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

pub(super) struct ProfileDocumentInput<'a, T> {
    pub id: &'a str,
    pub owner: &'a SubjectId,
    pub visibility: ProfileVisibility,
    pub kind: &'a str,
    pub draft_version: u64,
    pub updated_at: &'a str,
    pub retired_at: Option<&'a String>,
    pub record: &'a T,
}

pub(super) fn profile_document<T: Serialize>(
    input: &ProfileDocumentInput<'_, T>,
) -> Result<Document, ProfileStoreError> {
    Ok(doc! {
        "_id": input.id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": input.owner.as_str(),
        "visibility": record_bson(&input.visibility).map_err(ProfileStoreError::Corrupt)?,
        "kind": input.kind,
        "draft_version": signed(input.draft_version, "profile draft version")
            .map_err(ProfileStoreError::Corrupt)?,
        "last_revision": 0_i64,
        "updated_at": input.updated_at,
        "retired_at": nullable(input.retired_at),
        "record": record_bson(input.record).map_err(ProfileStoreError::Corrupt)?,
    })
}

pub(super) fn revision_document<T: Serialize>(
    profile_id: &str,
    revision: u32,
    record: &T,
) -> Result<Document, ProfileStoreError> {
    Ok(doc! {
        "_id": format!("{profile_id}:{revision:010}"),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "profile_id": profile_id,
        "revision": i64::from(revision),
        "record": record_bson(record).map_err(ProfileStoreError::Corrupt)?,
    })
}

pub(super) fn decode_record<T: DeserializeOwned>(
    document: &Document,
) -> Result<T, ProfileStoreError> {
    read_record(document).map_err(ProfileStoreError::Corrupt)
}

pub(super) fn replacement_fields<T: Serialize>(
    input: &ProfileDocumentInput<'_, T>,
) -> Result<Document, ProfileStoreError> {
    let mut document = profile_document(input)?;
    document.remove("_id");
    document.remove("last_revision");
    Ok(document)
}

pub(super) async fn save_draft<T: Serialize>(
    collection: &Collection<Document>,
    expected: u64,
    input: ProfileDocumentInput<'_, T>,
) -> Result<(), ProfileStoreError> {
    if input.draft_version != expected.checked_add(1).ok_or(ProfileStoreError::Conflict)? {
        return Err(ProfileStoreError::Conflict);
    }
    let result = collection
        .update_one(
            doc! {
                "_id": input.id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "draft_version": signed(expected, "expected draft version")
                    .map_err(|_| ProfileStoreError::Conflict)?,
            },
            doc! { "$set": replacement_fields(&input)? },
        )
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                ProfileStoreError::Duplicate
            } else {
                ProfileStoreError::Unavailable(error.to_string())
            }
        })?;
    if result.matched_count == 1 {
        return Ok(());
    }
    Err(classify_profile_miss(collection, input.id).await?)
}

pub(super) async fn classify_profile_miss(
    collection: &Collection<Document>,
    id: &str,
) -> Result<ProfileStoreError, ProfileStoreError> {
    match collection
        .find_one(doc! { "_id": id })
        .projection(doc! { "document_schema_version": 1 })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
    {
        None => Ok(ProfileStoreError::NotFound),
        Some(document) => {
            require_version(&document).map_err(ProfileStoreError::Corrupt)?;
            Ok(ProfileStoreError::Conflict)
        }
    }
}

pub(super) async fn retire_profile(
    collection: &Collection<Document>,
    id: &str,
    owner: &SubjectId,
    retired_at: String,
) -> Result<(), ProfileStoreError> {
    let result = collection
        .update_one(
            doc! {
                "_id": id,
                "owner": owner.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            },
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
        Ok(())
    } else {
        Err(ProfileStoreError::NotFound)
    }
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
    let mut filter = Document::new();
    if let Some(owner) = owner {
        filter.insert("owner", owner.as_str());
    }
    if public_only {
        filter.insert("visibility", "public");
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
) -> Result<(Document, u32), PublishError> {
    let profile = collection
        .find_one_and_update(
            doc! {
                "_id": id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "draft_version": signed(expected, "expected profile draft version")
                    .map_err(ProfileStoreError::Corrupt)
                    .map_err(PublishError::Domain)?,
                "retired_at": Bson::Null,
            },
            doc! { "$inc": { "last_revision": 1_i64 } },
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

pub(super) async fn readable_revision(
    revisions: &Collection<Document>,
    profiles: &Collection<Document>,
    profile_id: &str,
    revision: u32,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<Document, PublishError> {
    let revision = revisions
        .find_one(doc! {
            "profile_id": profile_id,
            "revision": i64::from(revision),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        })
        .session(&mut *session)
        .await
        .map_err(PublishError::Driver)?
        .ok_or(PublishError::Domain(
            ProfileStoreError::ReferenceUnavailable,
        ))?;
    let readable = profiles
        .find_one(doc! {
            "_id": profile_id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "$or": [
                { "owner": owner.as_str() },
                { "visibility": "public" },
            ],
        })
        .projection(doc! { "_id": 1 })
        .session(session)
        .await
        .map_err(PublishError::Driver)?
        .is_some();
    if readable {
        Ok(revision)
    } else {
        Err(PublishError::Domain(
            ProfileStoreError::ReferenceUnavailable,
        ))
    }
}
