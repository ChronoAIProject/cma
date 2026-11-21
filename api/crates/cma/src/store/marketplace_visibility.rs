use futures_util::TryStreamExt;
use layer2_core::ids::SubjectId;
use layer2_core::profiles::{
    MarketplaceError, MarketplaceLocator, ProfileCapacityBounds, ProfileVisibility,
};
use mongodb::bson::{doc, Document};

use super::marketplace_support::{
    collections_for_locator, decode_profile, decode_revision, driver_error, publication_metadata,
    ProfileCollections, StoredRevision, SYSTEM_OWNER,
};
use super::sandbox_mongo::{record_bson, MongoProfileStore, DOCUMENT_SCHEMA_VERSION};

const VISIBILITY_ATTEMPTS: usize = 5;

fn validation_error(error: layer2_core::profiles::ProfileError) -> MarketplaceError {
    MarketplaceError::Validation(error.code.into())
}

async fn validate_revisions(
    target: ProfileCollections<'_>,
    id: &str,
    expected_revision: u32,
    expected_published_at: Option<&str>,
    bounds: ProfileCapacityBounds,
    scanner: &dyn layer2_core::profiles::PublicValueScanner,
) -> Result<(), MarketplaceError> {
    if expected_revision == 0 {
        return Err(MarketplaceError::Unpublished);
    }
    let mut documents = target
        .revisions
        .find(doc! { "profile_id": id })
        .sort(doc! { "revision": 1 })
        .await
        .map_err(driver_error)?;
    let mut seen = 0_u32;
    let mut latest_published_at = None;
    while let Some(document) = documents.try_next().await.map_err(driver_error)? {
        let revision = decode_revision(target.kind, &document)?;
        let next = seen.checked_add(1).ok_or_else(|| {
            MarketplaceError::Corrupt("profile revision sequence overflowed".into())
        })?;
        if revision.revision() != next {
            return Err(MarketplaceError::Corrupt(
                "published profile revisions are not contiguous".into(),
            ));
        }
        match &revision {
            StoredRevision::Agent(revision) => {
                revision.content.validate().map_err(validation_error)?;
            }
            StoredRevision::Workspace(revision) => {
                revision.content.validate().map_err(validation_error)?;
            }
            StoredRevision::Sandbox(revision) => {
                revision
                    .content
                    .validate(bounds)
                    .map_err(validation_error)?;
                revision
                    .content
                    .validate_public_values(scanner)
                    .await
                    .map_err(MarketplaceError::PublicValues)?;
            }
        }
        seen = next;
        latest_published_at = Some(revision.published_at().to_string());
    }
    if seen != expected_revision || latest_published_at.as_deref() != expected_published_at {
        return Err(MarketplaceError::Corrupt(
            "profile publication metadata disagrees with its revisions".into(),
        ));
    }
    Ok(())
}

async fn owned_profile(
    target: ProfileCollections<'_>,
    id: &str,
    owner: &SubjectId,
) -> Result<(Document, super::marketplace_support::StoredProfile), MarketplaceError> {
    let document = target
        .profiles
        .find_one(doc! { "_id": id, "owner": owner.as_str() })
        .await
        .map_err(driver_error)?
        .ok_or(MarketplaceError::NotFound)?;
    let profile = decode_profile(target.kind, &document)?;
    Ok((document, profile))
}

pub(super) async fn set_profile_visibility(
    store: &MongoProfileStore,
    owner: &SubjectId,
    locator: &MarketplaceLocator,
    visibility: ProfileVisibility,
    updated_at: String,
    bounds: ProfileCapacityBounds,
    scanner: &dyn layer2_core::profiles::PublicValueScanner,
) -> Result<(), MarketplaceError> {
    if owner.as_str() == SYSTEM_OWNER {
        return Err(MarketplaceError::Forbidden);
    }
    let target = collections_for_locator(store, locator);
    for attempt in 0..VISIBILITY_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let (document, profile) = owned_profile(target, locator.as_str(), owner).await?;
        if profile.internal() {
            return Err(MarketplaceError::NotFound);
        }
        let publication = publication_metadata(&document)?;
        if visibility == ProfileVisibility::Public {
            validate_revisions(
                target,
                locator.as_str(),
                publication.revision,
                publication.published_at.as_deref(),
                bounds,
                scanner,
            )
            .await?;
        }
        let current_visibility =
            record_bson(&profile.visibility()).map_err(MarketplaceError::Corrupt)?;
        let next_visibility = record_bson(&visibility).map_err(MarketplaceError::Corrupt)?;
        let mut changed = doc! {
            "visibility": next_visibility.clone(),
            "record.visibility": next_visibility,
            "updated_at": &updated_at,
            "record.updated_at": &updated_at,
        };
        if matches!(locator, MarketplaceLocator::Sandbox(_))
            && visibility != ProfileVisibility::Public
        {
            changed.insert("record.availability.warmup_enabled", false);
            // Older Profiles have no policy object; materialize all defaults.
            if let super::marketplace_support::StoredProfile::Sandbox(profile) = &profile {
                changed.insert(
                    "record.availability.warmup_count",
                    i32::from(profile.availability.warmup_count),
                );
                changed.insert(
                    "record.availability.always_keep_alive",
                    profile.availability.always_keep_alive,
                );
            }
        }
        let result = target
            .profiles
            .update_one(
                doc! {
                    "_id": locator.as_str(),
                    "owner": owner.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "last_revision": i64::from(publication.revision),
                    "visibility": current_visibility.clone(),
                    "record.visibility": current_visibility,
                    "draft_version": document.get_i64("draft_version").map_err(|error| MarketplaceError::Corrupt(error.to_string()))?,
                },
                doc! { "$set": changed },
            )
            .await
            .map_err(driver_error)?;
        if result.matched_count == 1 {
            return Ok(());
        }
    }
    Err(MarketplaceError::Unavailable(
        "profile kept changing while visibility was updated".into(),
    ))
}
