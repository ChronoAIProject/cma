use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::TryStreamExt;
use layer2_core::profiles::{
    MarketplaceEntry, MarketplaceError, MarketplaceKind, MarketplaceLocator, MarketplacePage,
    MarketplaceQuery, ResolvedProfileRevision,
};
use mongodb::bson::{doc, Bson};
use serde::{Deserialize, Serialize};

use super::marketplace_support::{
    collections_for_kind, collections_for_locator, decode_profile, driver_error,
    find_public_profile, find_revision, marketplace_summary, publication_metadata,
    ProfileCollections, StoredRevision,
};
use super::sandbox_mongo::{MongoProfileStore, DOCUMENT_SCHEMA_VERSION};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceCursor {
    published_at: String,
    name: String,
    id: String,
}

impl From<&layer2_core::profiles::MarketplaceSummary> for MarketplaceCursor {
    fn from(summary: &layer2_core::profiles::MarketplaceSummary) -> Self {
        Self {
            published_at: summary.published_at.clone(),
            name: summary.name.clone(),
            id: summary.id.clone(),
        }
    }
}

fn encode_cursor(cursor: &MarketplaceCursor) -> Result<String, MarketplaceError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|error| MarketplaceError::Unavailable(format!("encode marketplace page: {error}")))
}

fn decode_cursor(value: &str) -> Result<MarketplaceCursor, MarketplaceError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| MarketplaceError::InvalidPage)
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|_| MarketplaceError::InvalidPage))
}

fn cursor_filter(cursor: &MarketplaceCursor) -> Bson {
    Bson::Document(doc! {
        "$or": [
            { "latest_published_at": { "$lt": &cursor.published_at } },
            {
                "latest_published_at": &cursor.published_at,
                "name": { "$gt": &cursor.name },
            },
            {
                "latest_published_at": &cursor.published_at,
                "name": &cursor.name,
                "_id": { "$gt": &cursor.id },
            },
        ]
    })
}

async fn query_collection(
    store: &MongoProfileStore,
    target: ProfileCollections<'_>,
    query: &MarketplaceQuery,
    cursor: Option<&MarketplaceCursor>,
) -> Result<Vec<layer2_core::profiles::MarketplaceSummary>, MarketplaceError> {
    let mut filter = doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "visibility": "public",
        "record.internal": { "$ne": true },
        "retired_at": Bson::Null,
        "last_revision": { "$gt": 0_i64 },
        "latest_published_at": { "$type": "string" },
    };
    if let Some(text) = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        filter.insert("$text", doc! { "$search": text });
    }
    if let Some(cursor) = cursor {
        filter.insert("$and", [cursor_filter(cursor)]);
    }
    let documents: Vec<_> = target
        .profiles
        .find(filter)
        .sort(doc! { "latest_published_at": -1, "name": 1, "_id": 1 })
        .limit(i64::from(query.limit) + 1)
        .await
        .map_err(driver_error)?
        .try_collect()
        .await
        .map_err(driver_error)?;
    let owners = documents
        .iter()
        .map(|document| {
            decode_profile(target.kind, document).map(|profile| profile.owner().to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let authors = super::directory::public_display_names(
        &store.client,
        &target.profiles.namespace().db,
        &owners,
    )
    .await
    .map_err(|_| {
        MarketplaceError::Unavailable("profile author names could not be loaded".into())
    })?;
    documents
        .iter()
        .map(|document| marketplace_summary(target.kind, document, &authors))
        .collect()
}

pub(super) async fn list_marketplace(
    store: &MongoProfileStore,
    query: &MarketplaceQuery,
) -> Result<MarketplacePage, MarketplaceError> {
    if query.limit == 0 || query.limit > 50 {
        return Err(MarketplaceError::InvalidPage);
    }
    let cursor = query.cursor.as_deref().map(decode_cursor).transpose()?;
    let mut rows = Vec::new();
    for kind in [
        MarketplaceKind::Agent,
        MarketplaceKind::Workspace,
        MarketplaceKind::Sandbox,
    ] {
        if query.kind.is_none_or(|selected| selected == kind) {
            rows.extend(
                query_collection(
                    store,
                    collections_for_kind(store, kind),
                    query,
                    cursor.as_ref(),
                )
                .await?,
            );
        }
    }
    rows.sort_by(|left, right| {
        right
            .published_at
            .cmp(&left.published_at)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    let has_more = rows.len() > usize::from(query.limit);
    rows.truncate(usize::from(query.limit));
    let next = if has_more {
        rows.last()
            .map(MarketplaceCursor::from)
            .as_ref()
            .map(encode_cursor)
            .transpose()?
    } else {
        None
    };
    Ok(MarketplacePage { items: rows, next })
}

async fn resolve_revision(
    store: &MongoProfileStore,
    kind: MarketplaceKind,
    id: &str,
    revision: u32,
) -> Result<ResolvedProfileRevision, MarketplaceError> {
    let target = collections_for_kind(store, kind);
    let document = target
        .profiles
        .find_one(doc! { "_id": id })
        .await
        .map_err(driver_error)?
        .ok_or_else(|| corrupt_reference("profile is missing"))?;
    let profile = decode_profile(kind, &document)?;
    let revision = find_revision(target, id, revision)
        .await
        .map_err(|error| match error {
            MarketplaceError::NotFound => corrupt_reference("profile revision is missing"),
            other => other,
        })?;
    Ok(ResolvedProfileRevision {
        owner: profile.owner().clone(),
        name: profile.name().to_string(),
        revision: revision.into_marketplace(),
    })
}

async fn resolved_revisions(
    store: &MongoProfileStore,
    revision: &StoredRevision,
) -> Result<Vec<ResolvedProfileRevision>, MarketplaceError> {
    let mut revisions = Vec::new();
    match revision {
        StoredRevision::Agent(_) => {}
        StoredRevision::Workspace(revision) => {
            for reference in &revision.content.agent_profiles {
                revisions.push(
                    resolve_revision(
                        store,
                        MarketplaceKind::Agent,
                        reference.id.as_str(),
                        reference.revision,
                    )
                    .await?,
                );
            }
        }
        StoredRevision::Sandbox(revision) => {
            for reference in &revision.content.workspace_profiles {
                revisions.push(
                    resolve_revision(
                        store,
                        MarketplaceKind::Workspace,
                        reference.id.as_str(),
                        reference.revision,
                    )
                    .await?,
                );
            }
        }
    }
    Ok(revisions)
}

fn corrupt_reference(detail: &str) -> MarketplaceError {
    MarketplaceError::Corrupt(format!("referenced marketplace {detail}"))
}

pub(super) async fn read_marketplace(
    store: &MongoProfileStore,
    locator: &MarketplaceLocator,
) -> Result<MarketplaceEntry, MarketplaceError> {
    let target = collections_for_locator(store, locator);
    let (document, profile) = find_public_profile(target, locator.as_str()).await?;
    let publication = publication_metadata(&document)?;
    if publication.revision == 0 {
        return Err(MarketplaceError::NotFound);
    }
    let authors = super::directory::public_display_names(
        &store.client,
        &target.profiles.namespace().db,
        &[profile.owner().to_string()],
    )
    .await
    .map_err(|_| {
        MarketplaceError::Unavailable("profile author names could not be loaded".into())
    })?;
    let summary = marketplace_summary(target.kind, &document, &authors)?;
    let revision = find_revision(target, locator.as_str(), publication.revision).await?;
    if publication.published_at.as_deref() != Some(revision.published_at()) {
        return Err(MarketplaceError::Corrupt(
            "latest revision and publication time disagree".into(),
        ));
    }
    let resolved_revisions = resolved_revisions(store, &revision).await?;
    let resolved_profiles = resolved_revisions
        .iter()
        .map(ResolvedProfileRevision::resolved_name)
        .collect();
    Ok(MarketplaceEntry {
        owner: profile.owner().clone(),
        summary,
        revision: revision.into_marketplace(),
        resolved_profiles,
        resolved_revisions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_is_opaque_and_rejects_malformed_values() {
        let cursor = MarketplaceCursor {
            published_at: "2026-09-05T00:00:00Z".into(),
            name: "name|with|separators".into(),
            id: "agp_one".into(),
        };
        let encoded = encode_cursor(&cursor).unwrap();
        let decoded = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded.published_at, cursor.published_at);
        assert_eq!(decoded.name, cursor.name);
        assert_eq!(decoded.id, cursor.id);
        assert_eq!(
            decode_cursor("not base64").unwrap_err(),
            MarketplaceError::InvalidPage
        );
    }
}
