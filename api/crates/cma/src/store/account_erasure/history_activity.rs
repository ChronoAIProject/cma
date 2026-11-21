//! Actor attribution supplies organization candidates, never personal ownership.
use super::{Context, Error, Set, PAGE};
use crate::store::durability::majority_collection;
use futures_util::TryStreamExt;
use layer2_core::resource_access::ResourceOwner;
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};
use std::collections::BTreeMap;

pub(super) async fn protect_organizations(
    db: &Database,
    contexts: &mut [Context],
) -> Result<(), Error> {
    let subjects: Vec<_> = contexts
        .iter()
        .map(|context| context.subject.clone())
        .collect();
    let activity = majority_collection::<Document>(db, "activity");
    let policies = majority_collection::<Document>(db, "resource_policies");
    let mut after: Option<(String, Bson, Bson)> = None;
    loop {
        let mut filter = doc! {
            "record.actor":{"$in":&subjects}, "record.target.kind":"resource"
        };
        if let Some((actor, recorded_at, id)) = &after {
            filter.insert(
                "$or",
                vec![
                    doc! {"record.actor":{"$gt":actor}},
                    doc! {"record.actor":actor,"recorded_at":{"$lt":recorded_at}},
                    doc! {"record.actor":actor,"recorded_at":recorded_at,"_id":{"$lt":id}},
                ],
            );
        }
        // Follow the existing actor/time/identity index in its exact order.
        // This discovery is separate from the shared unindexed erasure scan;
        // every candidate page serves the whole cohort.
        let rows: Vec<_> = activity
            .find(filter)
            .projection(doc! {"_id":1,"recorded_at":1,"record.actor":1,"record.target":1})
            .sort(doc! {"record.actor":1,"recorded_at":-1,"_id":-1})
            .limit(PAGE)
            .batch_size(PAGE as u32)
            .await?
            .try_collect()
            .await?;
        let mut candidates: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for row in &rows {
            let Some(id) = super::super::super::identity_activity::sandbox_target(row) else {
                continue;
            };
            for (index, context) in contexts.iter().enumerate() {
                if super::super::super::selectors::strings(row, "record.actor")
                    .contains(&context.subject.as_str())
                {
                    candidates.entry(id.to_owned()).or_default().push(index);
                }
            }
        }
        if !candidates.is_empty() {
            let ids: Vec<_> = candidates.keys().cloned().collect();
            let mut cursor = policies
                .find(doc! {
                    "_id":{"$in":ids},"owner_kind":"organization"
                })
                .batch_size(PAGE as u32)
                .await?;
            while let Some(row) = cursor.try_next().await? {
                let subject = row
                    .get_str("_id")
                    .ok()
                    .and_then(|id| candidates.get(id))
                    .and_then(|indexes| indexes.first())
                    .map_or("", |index| contexts[*index].subject.as_str());
                let policy = crate::store::resource_access::records::read_policy(&row)
                    .map_err(|_| Error::InvalidRecord("resource_policies").attribute(subject))?;
                if !matches!(policy.owner, ResourceOwner::Organization { .. }) {
                    continue;
                }
                let id = row
                    .get_str("_id")
                    .map_err(|_| Error::InvalidRecord("resource_policies"))?;
                for index in candidates.get(id).into_iter().flatten() {
                    contexts[*index].protect(Set::Sandboxes, id);
                    contexts[*index]
                        .organization_sandboxes
                        .insert(id.to_owned());
                    contexts[*index].bounds()?;
                }
            }
        }
        if rows.len() < PAGE as usize {
            break;
        }
        let row = rows.last().ok_or(Error::InvalidRecord("activity"))?;
        let attributed = contexts
            .iter()
            .find(|context| {
                super::super::super::selectors::strings(row, "record.actor")
                    .contains(&context.subject.as_str())
            })
            .map_or("", |context| context.subject.as_str());
        let actor = super::super::value(row, "record.actor")
            .and_then(Bson::as_str)
            .ok_or_else(|| Error::InvalidRecord("activity").attribute(attributed))?;
        let recorded_at = row
            .get("recorded_at")
            .filter(|value| matches!(value, Bson::DateTime(_)))
            .ok_or_else(|| Error::InvalidRecord("activity").attribute(actor))?;
        let id = row
            .get("_id")
            .ok_or_else(|| Error::InvalidRecord("activity").attribute(actor))?;
        after = Some((actor.to_owned(), recorded_at.clone(), id.clone()));
    }
    Ok(())
}
