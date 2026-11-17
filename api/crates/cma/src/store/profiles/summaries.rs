//! Snapshot-bounded owner pages with indexed immutable publication evidence.

use super::*;
use layer2_core::profiles::store::{ProfileSummary, PublishedProfileSummary};
use mongodb::options::ReadConcern;
use mongodb::Client;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(15);

pub(in crate::store) struct SummaryReader<'a, P> {
    pub client: &'a Client,
    pub profiles: &'a Collection<Document>,
    pub revisions: &'a Collection<Document>,
    pub profile: fn(&Document) -> Result<P, ProfileStoreError>,
    pub revision: fn(&Document) -> Result<PublishedProfileSummary, ProfileStoreError>,
}

pub(in crate::store) fn pipeline(
    revisions: &str,
    owner: &SubjectId,
    page: &ProfilePageRequest,
) -> Vec<Document> {
    let mut filter = doc! { "owner": owner.as_str(), "record.internal": { "$ne": true } };
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    vec![
        doc! { "$match": filter },
        doc! { "$sort": { "_id": 1 } },
        doc! { "$limit": i64::from(page.limit) + 1 },
        doc! { "$replaceWith": { "profile": "$$ROOT" } },
        doc! { "$lookup": {
            "from": revisions,
            // Use query equality, as the original revision find does: an
            // array-valued foreign key must still match and fail source validation.
            "localField": "profile._id",
            "foreignField": "profile_id",
            "pipeline": [
                { "$sort": { "revision": -1 } },
                // Retain the existing latest-read lookahead integrity check.
                { "$limit": 2 },
                { "$replaceWith": { "source": "$$ROOT", "publication": true } },
            ],
            "as": "revisions",
        } },
        // Return source documents separately: joining large draft/revision
        // payloads into a single result could exceed Mongo's document limit.
        doc! { "$project": { "rows": { "$concatArrays": [
            [{ "source": "$profile", "publication": false }], "$revisions",
        ] } } },
        doc! { "$unwind": "$rows" },
        doc! { "$replaceWith": "$rows" },
    ]
}

impl<P> SummaryReader<'_, P> {
    pub async fn read(
        &self,
        owner: &SubjectId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<P>>, ProfileStoreError> {
        validate_page(page)?;
        tokio::time::timeout(DEADLINE, self.snapshot(owner, page))
            .await
            .map_err(|_| ProfileStoreError::Unavailable("profile summary read timed out".into()))?
    }

    async fn snapshot(
        &self,
        owner: &SubjectId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<P>>, ProfileStoreError> {
        let unavailable =
            |error: mongodb::error::Error| ProfileStoreError::Unavailable(error.to_string());
        let corrupt = |detail: &str| ProfileStoreError::Corrupt(detail.into());
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)?;
        let mut cursor = self
            .profiles
            .aggregate(pipeline(self.revisions.name(), owner, page))
            .read_concern(ReadConcern::snapshot())
            .max_time(DEADLINE)
            .batch_size(3 * (u32::from(page.limit) + 1))
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let mut stream = cursor.stream(&mut session);
        let mut items: Vec<ProfileSummary<P>> = Vec::new();
        let mut ids = Vec::new();
        let mut revision_count = 0;
        let mut previous_revision = None;
        while let Some(envelope) = stream.try_next().await.map_err(unavailable)? {
            let row = envelope
                .get_document("source")
                .map_err(|_| corrupt("profile summary source is missing"))?;
            if envelope
                .get_bool("publication")
                .map_err(|_| corrupt("profile summary source kind is missing"))?
            {
                revision_count += 1;
                if revision_count > 2 {
                    return Err(corrupt(
                        "profile summary revision is outside its source page",
                    ));
                }
                // The extra profile establishes only pagination. The prior
                // list path validated that profile, but never its revisions.
                if items.len() > usize::from(page.limit) {
                    continue;
                }
                if row.get_str("profile_id").ok() != ids.last().map(String::as_str) {
                    return Err(corrupt("profile summary revision has the wrong source"));
                }
                let summary = (self.revision)(row)?;
                if previous_revision.is_some_and(|number| number <= summary.revision) {
                    return Err(corrupt("profile summary revisions are not newest first"));
                }
                previous_revision = Some(summary.revision);
                if revision_count == 1 {
                    items
                        .last_mut()
                        .ok_or_else(|| corrupt("profile summary has no profile"))?
                        .latest_revision = Some(summary);
                }
            } else {
                let profile = (self.profile)(row)?;
                let id = row
                    .get_str("_id")
                    .map_err(|_| corrupt("profile summary id is missing"))?;
                if items.len() > usize::from(page.limit)
                    || row.get_str("owner").ok() != Some(owner.as_str())
                    || ids.last().is_some_and(|previous| previous.as_str() >= id)
                {
                    return Err(corrupt("profile summary is outside its owner page"));
                }
                ids.push(id.to_owned());
                items.push(ProfileSummary {
                    profile,
                    latest_revision: None,
                });
                revision_count = 0;
                previous_revision = None;
            }
        }
        let next = if items.len() > usize::from(page.limit) {
            items.pop();
            ids.pop();
            ids.pop()
        } else {
            None
        };
        Ok(ProfilePage { items, next })
    }
}
