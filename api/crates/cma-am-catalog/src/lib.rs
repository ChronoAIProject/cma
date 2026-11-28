//! Shared BSON encoding and draft CAS updates for profile aggregates.

use layer2_core::ids::SubjectId;
use layer2_core::profiles::store::ProfileStoreError;
use layer2_core::profiles::ProfileVisibility;
use mongodb::bson::{doc, Bson, Document};
use mongodb::Collection;
use serde::de::DeserializeOwned;
use serde::Serialize;

use cma_am_mongo_primitives::{nullable, read_record, record_bson, require_version, signed};

const DOCUMENT_SCHEMA_VERSION: i64 = 1;

mod retained_reader;

pub struct ProfileDocumentInput<'a, T> {
    pub id: &'a str,
    pub owner: &'a SubjectId,
    pub name: &'a str,
    pub description: &'a str,
    pub visibility: ProfileVisibility,
    pub kind: &'a str,
    pub uses_count: u64,
    pub draft_version: u64,
    pub updated_at: &'a str,
    pub retired_at: Option<&'a String>,
    pub record: &'a T,
}

pub fn profile_document<T: Serialize>(
    input: &ProfileDocumentInput<'_, T>,
) -> Result<Document, ProfileStoreError> {
    Ok(doc! {
        "_id": input.id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": input.owner.as_str(),
        "name": input.name,
        "description": input.description,
        "visibility": record_bson(&input.visibility).map_err(ProfileStoreError::Corrupt)?,
        "kind": input.kind,
        "uses_count": signed(input.uses_count, "profile use count")
            .map_err(ProfileStoreError::Corrupt)?,
        "draft_version": signed(input.draft_version, "profile draft version")
            .map_err(ProfileStoreError::Corrupt)?,
        "last_revision": 0_i64,
        "latest_published_at": Bson::Null,
        "updated_at": input.updated_at,
        "retired_at": nullable(input.retired_at),
        "record": record_bson(input.record).map_err(ProfileStoreError::Corrupt)?,
    })
}

pub fn revision_document<T: Serialize>(
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

pub fn decode_record<T: DeserializeOwned>(document: &Document) -> Result<T, ProfileStoreError> {
    let hydrated = retained_reader::hydrate(document)?;
    read_record(
        hydrated.as_ref().unwrap_or(document),
        DOCUMENT_SCHEMA_VERSION,
    )
    .map_err(ProfileStoreError::Corrupt)
}

pub fn is_retained_profile(document: &Document) -> Result<bool, ProfileStoreError> {
    retained_reader::is_retained(document)
}

fn aggregate_document<T: Serialize>(record: &T) -> Result<Document, ProfileStoreError> {
    match record_bson(record).map_err(ProfileStoreError::Corrupt)? {
        Bson::Document(document) => Ok(document),
        _ => Err(ProfileStoreError::Corrupt(
            "profile aggregate did not encode as a document".into(),
        )),
    }
}

fn required_field(record: &Document, name: &str) -> Result<Bson, ProfileStoreError> {
    record
        .get(name)
        .cloned()
        .ok_or_else(|| ProfileStoreError::Corrupt(format!("profile aggregate has no {name}")))
}

fn optional_field(record: &Document, name: &str) -> Bson {
    record.get(name).cloned().unwrap_or(Bson::Null)
}

pub async fn save_draft<T: Serialize>(
    collection: &Collection<Document>,
    expected: u64,
    input: ProfileDocumentInput<'_, T>,
) -> Result<(), ProfileStoreError> {
    let (filter, update) = draft_mutation(expected, &input)?;
    let result = collection
        .update_one(filter, update)
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    if result.matched_count == 1 {
        return Ok(());
    }
    Err(classify_profile_miss(collection, input.id).await?)
}

pub fn draft_mutation<T: Serialize>(
    expected: u64,
    input: &ProfileDocumentInput<'_, T>,
) -> Result<(Document, Document), ProfileStoreError> {
    let next = expected.checked_add(1).ok_or(ProfileStoreError::Conflict)?;
    if input.draft_version != next || input.retired_at.is_some() {
        return Err(ProfileStoreError::Conflict);
    }

    let record = aggregate_document(input.record)?;
    let expected =
        signed(expected, "expected draft version").map_err(|_| ProfileStoreError::Conflict)?;
    let uses_count =
        signed(input.uses_count, "profile use count").map_err(|_| ProfileStoreError::Conflict)?;
    let visibility = record_bson(&input.visibility).map_err(ProfileStoreError::Corrupt)?;
    let mut filter = doc! {
        "_id": input.id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": input.owner.as_str(),
        "visibility": visibility,
        "kind": input.kind,
        "uses_count": uses_count,
        "draft_version": expected,
        "record.draft_version": expected,
        "retired_at": Bson::Null,
        "account_erasure_fenced": { "$ne": true },
    };
    for name in ["id", "owner", "visibility", "uses_count", "created_at"] {
        filter.insert(format!("record.{name}"), required_field(&record, name)?);
    }
    // Missing marks legacy user profiles; a draft cannot change its origin.
    filter.insert("record.internal", optional_field(&record, "internal"));
    filter.insert("record.forked_from", optional_field(&record, "forked_from"));
    filter.insert("record.retired_at", Bson::Null);

    if input.kind == "sandbox"
        && record
            .get_document("availability")
            .is_ok_and(|policy| matches!(policy.get_bool("warmup_enabled"), Ok(true)))
    {
        filter.insert("last_revision", doc! { "$gt": 0_i64 });
    }
    let mut update = doc! {
        "$set": {
            "name": input.name,
            "description": input.description,
            "draft_version": signed(input.draft_version, "profile draft version")
                .map_err(|_| ProfileStoreError::Conflict)?,
            "updated_at": input.updated_at,
            "record.name": required_field(&record, "name")?,
            "record.description": required_field(&record, "description")?,
            "record.draft": required_field(&record, "draft")?,
            "record.draft_version": required_field(&record, "draft_version")?,
            "record.updated_at": required_field(&record, "updated_at")?,
        }
    };
    if input.kind == "sandbox" {
        update
            .get_document_mut("$set")
            .map_err(|error| ProfileStoreError::Corrupt(error.to_string()))?
            .insert(
                "record.availability",
                required_field(&record, "availability")?,
            );
    }
    Ok((filter, update))
}

async fn classify_profile_miss(
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
            require_version(&document, DOCUMENT_SCHEMA_VERSION)
                .map_err(ProfileStoreError::Corrupt)?;
            Ok(ProfileStoreError::Conflict)
        }
    }
}
