use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::TryStreamExt;
use layer2_core::profiles::{
    MarketplaceEntry, MarketplaceError, MarketplaceKind, MarketplaceLocator, MarketplacePage,
    MarketplaceQuery, ResolvedProfileName,
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
    target: ProfileCollections<'_>,
    query: &MarketplaceQuery,
    cursor: Option<&MarketplaceCursor>,
) -> Result<Vec<layer2_core::profiles::MarketplaceSummary>, MarketplaceError> {
    let mut filter = doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "visibility": "public",
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
    let mut documents = target
        .profiles
        .find(filter)
        .sort(doc! { "latest_published_at": -1, "name": 1, "_id": 1 })
        .limit(i64::from(query.limit) + 1)
        .await
        .map_err(driver_error)?;
    let mut rows = Vec::new();
    while let Some(document) = documents.try_next().await.map_err(driver_error)? {
        rows.push(marketplace_summary(target.kind, &document)?);
    }
    Ok(rows)
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
                query_collection(collections_for_kind(store, kind), query, cursor.as_ref()).await?,
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

async fn resolve_name(
    store: &MongoProfileStore,
    kind: MarketplaceKind,
    id: &str,
    revision: u32,
) -> Result<Option<ResolvedProfileName>, MarketplaceError> {
    let target = collections_for_kind(store, kind);
    let Some(document) = target
        .profiles
        .find_one(doc! { "_id": id })
        .await
        .map_err(driver_error)?
    else {
        return Ok(None);
    };
    let profile = decode_profile(kind, &document)?;
    Ok(Some(ResolvedProfileName {
        kind,
        id: id.to_string(),
        revision,
        name: profile.name().to_string(),
    }))
}

async fn resolved_names(
    store: &MongoProfileStore,
    revision: &StoredRevision,
) -> Result<Vec<ResolvedProfileName>, MarketplaceError> {
    let mut names = Vec::new();
    match revision {
        StoredRevision::Agent(_) => {}
        StoredRevision::Workspace(revision) => {
            for reference in &revision.content.agent_profiles {
                if let Some(name) = resolve_name(
                    store,
                    MarketplaceKind::Agent,
                    reference.id.as_str(),
                    reference.revision,
                )
                .await?
                {
                    names.push(name);
                }
            }
        }
        StoredRevision::Sandbox(revision) => {
            for reference in &revision.content.workspace_profiles {
                if let Some(name) = resolve_name(
                    store,
                    MarketplaceKind::Workspace,
                    reference.id.as_str(),
                    reference.revision,
                )
                .await?
                {
                    names.push(name);
                }
            }
        }
    }
    Ok(names)
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
    let summary = marketplace_summary(target.kind, &document)?;
    let revision = find_revision(target, locator.as_str(), publication.revision).await?;
    if publication.published_at.as_deref() != Some(revision.published_at()) {
        return Err(MarketplaceError::Corrupt(
            "latest revision and publication time disagree".into(),
        ));
    }
    let resolved_profiles = resolved_names(store, &revision).await?;
    Ok(MarketplaceEntry {
        owner: profile.owner().clone(),
        summary,
        revision: revision.into_marketplace(),
        resolved_profiles,
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
