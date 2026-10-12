//! Mongo operator inventory and retirement for all profile kinds.

use futures_util::TryStreamExt;
use layer2_core::profiles::store::{ProfilePage, ProfileStoreError};
use layer2_core::profiles::{
    ProfileAdminQuery, ProfileAdminStore, ProfileAdminSummary, ProfileKind, ProfileLocator,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::ReturnDocument;
use mongodb::Collection;

use super::profiles::MAX_PROFILE_PAGE;
use super::sandbox_mongo::{require_version, MongoProfileStore, DOCUMENT_SCHEMA_VERSION};

fn admin_filter(query: &ProfileAdminQuery) -> Document {
    let mut filter = doc! { "record.internal": { "$ne": true } };
    if let Some(owner) = &query.owner {
        filter.insert("owner", owner.as_str());
    }
    if let Some(visibility) = query.visibility {
        filter.insert(
            "visibility",
            match visibility {
                layer2_core::profiles::ProfileVisibility::Private => "private",
                layer2_core::profiles::ProfileVisibility::Public => "public",
            },
        );
    }
    if let Some(cursor) = &query.page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    filter
}

async fn documents(
    collection: &Collection<Document>,
    query: &ProfileAdminQuery,
) -> Result<Vec<Document>, ProfileStoreError> {
    let mut cursor = collection
        .find(admin_filter(query))
        .sort(doc! { "_id": 1 })
        .limit(i64::from(query.page.limit) + 1)
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    let mut rows = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
    {
        rows.push(document);
    }
    Ok(rows)
}

fn decode_agent(document: &Document) -> Result<ProfileAdminSummary, ProfileStoreError> {
    super::agent_profiles::read_profile(document).map(|profile| ProfileAdminSummary::from(&profile))
}

fn decode_workspace(document: &Document) -> Result<ProfileAdminSummary, ProfileStoreError> {
    super::workspace_profiles::read_profile(document)
        .map(|profile| ProfileAdminSummary::from(&profile))
}

fn decode_sandbox(document: &Document) -> Result<ProfileAdminSummary, ProfileStoreError> {
    super::sandbox_profiles::read_profile(document)
        .map(|profile| ProfileAdminSummary::from(&profile))
}

async fn append_kind(
    rows: &mut Vec<ProfileAdminSummary>,
    collection: &Collection<Document>,
    query: &ProfileAdminQuery,
    decode: fn(&Document) -> Result<ProfileAdminSummary, ProfileStoreError>,
) -> Result<(), ProfileStoreError> {
    for document in documents(collection, query).await? {
        rows.push(decode(&document)?);
    }
    Ok(())
}

enum RetirementDocument {
    Updated(Document),
    Existing(Document),
}

fn retirement_update(retired_at: &str) -> Document {
    doc! {
        "$set": {
            "updated_at": retired_at,
            "retired_at": retired_at,
            "record.updated_at": retired_at,
            "record.retired_at": retired_at,
        }
    }
}

fn retirement_filter(id: &str, kind: &str) -> Document {
    doc! {
        "_id": id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "kind": kind,
        "record.internal": { "$ne": true },
        "retired_at": Bson::Null,
    }
}

async fn retire_document(
    collection: &Collection<Document>,
    id: &str,
    kind: &str,
    retired_at: &str,
) -> Result<RetirementDocument, ProfileStoreError> {
    let updated = collection
        .find_one_and_update(retirement_filter(id, kind), retirement_update(retired_at))
        .return_document(ReturnDocument::After)
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    if let Some(document) = updated {
        return Ok(RetirementDocument::Updated(document));
    }
    let existing = collection
        .find_one(doc! { "_id": id, "record.internal": { "$ne": true } })
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?
        .ok_or(ProfileStoreError::NotFound)?;
    require_version(&existing).map_err(ProfileStoreError::Corrupt)?;
    Ok(RetirementDocument::Existing(existing))
}

fn verify_retired(
    document: RetirementDocument,
    decode: fn(&Document) -> Result<ProfileAdminSummary, ProfileStoreError>,
) -> Result<ProfileAdminSummary, ProfileStoreError> {
    let (document, must_already_be_retired) = match document {
        RetirementDocument::Updated(document) => (document, false),
        RetirementDocument::Existing(document) => (document, true),
    };
    let summary = decode(&document)?;
    if must_already_be_retired && summary.retired_at.is_none() {
        return Err(ProfileStoreError::Conflict);
    }
    Ok(summary)
}

#[async_trait::async_trait]
impl ProfileAdminStore for MongoProfileStore {
    async fn list_profiles(
        &self,
        query: &ProfileAdminQuery,
    ) -> Result<ProfilePage<ProfileAdminSummary>, ProfileStoreError> {
        if query.page.limit == 0 || query.page.limit > MAX_PROFILE_PAGE {
            return Err(layer2_core::profiles::ProfileError::new("invalid_page", "limit").into());
        }
        let mut rows = Vec::new();
        if query.kind.is_none_or(|kind| kind == ProfileKind::Agent) {
            append_kind(&mut rows, &self.agent_profiles, query, decode_agent).await?;
        }
        if query.kind.is_none_or(|kind| kind == ProfileKind::Workspace) {
            append_kind(&mut rows, &self.workspace_profiles, query, decode_workspace).await?;
        }
        if query.kind.is_none_or(|kind| kind == ProfileKind::Sandbox) {
            append_kind(&mut rows, &self.sandbox_profiles, query, decode_sandbox).await?;
        }
        rows.sort_by(|left, right| left.id.cmp(&right.id));
        let has_more = rows.len() > usize::from(query.page.limit);
        rows.truncate(usize::from(query.page.limit));
        let next = has_more.then(|| {
            rows.last()
                .expect("a bounded page has a last profile")
                .id
                .clone()
        });
        Ok(ProfilePage { items: rows, next })
    }

    async fn retire_profile(
        &self,
        locator: &ProfileLocator,
        retired_at: String,
    ) -> Result<ProfileAdminSummary, ProfileStoreError> {
        let protected = match locator {
            ProfileLocator::Agent(id) => id.as_str() == layer2_core::DEFAULT_AGENT_PROFILE_ID,
            ProfileLocator::Workspace(id) => {
                id.as_str() == layer2_core::DEFAULT_WORKSPACE_PROFILE_ID
            }
            ProfileLocator::Sandbox(id) => id.as_str() == layer2_core::DEFAULT_SANDBOX_PROFILE_ID,
        };
        if protected {
            return Err(ProfileStoreError::Protected);
        }
        let (collection, decode): (
            &Collection<Document>,
            fn(&Document) -> Result<ProfileAdminSummary, ProfileStoreError>,
        ) = match locator {
            ProfileLocator::Agent(_) => (&self.agent_profiles, decode_agent),
            ProfileLocator::Workspace(_) => (&self.workspace_profiles, decode_workspace),
            ProfileLocator::Sandbox(_) => (&self.sandbox_profiles, decode_sandbox),
        };
        let document = retire_document(
            collection,
            locator.as_str(),
            locator.kind().as_str(),
            &retired_at,
        )
        .await?;
        verify_retired(document, decode)
    }
}

#[cfg(test)]
mod tests {
    use layer2_core::ids::SubjectId;
    use layer2_core::profiles::store::ProfilePageRequest;
    use layer2_core::profiles::ProfileVisibility;

    use super::*;

    #[test]
    fn profile_inventory_filter_uses_exact_owner_visibility_and_cursor() {
        let query = ProfileAdminQuery {
            kind: None,
            visibility: Some(ProfileVisibility::Private),
            owner: Some(SubjectId("profile-owner".into())),
            page: ProfilePageRequest {
                limit: 20,
                cursor: Some("agp_after".into()),
            },
        };
        assert_eq!(
            admin_filter(&query),
            doc! {
                "record.internal": { "$ne": true },
                "owner": "profile-owner",
                "visibility": "private",
                "_id": { "$gt": "agp_after" },
            }
        );
    }

    #[test]
    fn retirement_update_changes_both_query_fields_and_aggregate() {
        assert_eq!(
            retirement_filter("agp_target", "agent"),
            doc! {
                "_id": "agp_target",
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "kind": "agent",
                "record.internal": { "$ne": true },
                "retired_at": Bson::Null,
            }
        );
        assert_eq!(
            retirement_update("2026-09-05T12:00:00Z"),
            doc! { "$set": {
                "updated_at": "2026-09-05T12:00:00Z",
                "retired_at": "2026-09-05T12:00:00Z",
                "record.updated_at": "2026-09-05T12:00:00Z",
                "record.retired_at": "2026-09-05T12:00:00Z",
            } }
        );
    }
}
