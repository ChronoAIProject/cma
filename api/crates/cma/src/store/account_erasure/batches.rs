//! Index-compatible bounded selection and final idempotent delete fences.
use super::{
    catalog::{Entry, Erasure},
    context::Context,
    guards,
    references::Redaction,
    selectors, *,
};
use crate::store::durability::majority_collection;
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};

use super::erase_source_log::erase_source_log;

pub(super) fn authored(name: &str, rule: Erasure) -> bool {
    matches!(name, "agent_message_feedback" | "agent_suggestions")
        && matches!(rule, Erasure::Owner(_))
}
pub(super) fn matches(entry: &Entry, rule: Erasure, row: &Document, context: &Context) -> bool {
    (!context.protects(row) || authored(entry.collection, rule))
        && selectors::matches(rule, row, context)
}
#[cfg(test)]
pub(super) async fn select(
    db: &Database,
    entry: &Entry,
    indexed: bool,
    contexts: &[Context],
    after: Option<&Bson>,
) -> Result<Vec<Document>, Error> {
    if indexed {
        Ok(super::indexed::page(db, entry, contexts, None, after)
            .await?
            .rows)
    } else {
        super::scan::page(db, entry.collection, after).await
    }
}
pub(super) struct DeletePage<'a> {
    pub rows: &'a [Document],
    pub ids: &'a [Bson],
}
pub(super) struct IdentityScope<'a> {
    pub subjects: &'a BTreeSet<String>,
    pub agents: &'a BTreeSet<String>,
}
pub(super) async fn remove(
    db: &Database,
    entry: &Entry,
    page: DeletePage<'_>,
    contexts: &[Context],
    now: &str,
    options: Options,
    batch: &mut Batch,
) -> Result<(), Error> {
    let DeletePage { rows, ids } = page;
    if ids.is_empty() {
        return Ok(());
    }
    if entry.collection == crate::store::directory::USERS && options.retain_legal_consents {
        use crate::store::retained_consents;
        retained_consents::prepare(db).await?;
        for id in ids {
            let subject = id.as_str().ok_or(Error::InvalidRecord(entry.collection))?;
            if retained_consents::erase_user(db, subject)
                .await
                .map_err(|error| error.attribute(subject))?
            {
                *batch.deleted.entry(entry.collection.into()).or_default() += 1;
            }
        }
        return Ok(());
    }
    let filter = match entry.collection {
        crate::store::agent_observation::COLLECTION => {
            // Lease creation writes its source Agent in the same transaction.
            // Once the source is gone, no delayed acquisition can recreate a
            // deleted observer. Retained sources are not erasure authority.
            let present: Vec<_> = majority_collection::<Document>(db, "agents")
                .find(doc! {"_id":{"$in":ids}})
                .projection(doc! {"_id":1})
                .limit(500)
                .await?
                .try_collect::<Vec<_>>()
                .await?;
            let retained: Vec<_> = present.iter().filter_map(|row| row.get("_id")).collect();
            doc! {"_id":{"$in":ids,"$nin":retained}}
        }
        "sandboxes" => guards::sandbox_filter(contexts, ids, now)?,
        "sandbox_usage_events" => {
            doc! {"$and":[{"_id":{"$in":ids}},guards::usage_filter(options.billing_enabled)]}
        }
        "sandbox_billing_state" => guards::billing_filter(rows, ids),
        _ => doc! {"_id":{"$in":ids}},
    };
    let removed = if entry.collection == "agent_log" {
        erase_source_log(db, rows, ids, filter).await?
    } else {
        majority_collection::<Document>(db, entry.collection)
            .delete_many(filter)
            .await?
            .deleted_count
    };
    if removed > 0 {
        *batch.deleted.entry(entry.collection.into()).or_default() += removed;
    }
    if removed == ids.len() as u64 {
        return Ok(());
    }
    let remaining = majority_collection::<Document>(db, entry.collection)
        .find(doc! {"_id":{"$in":ids}})
        .limit(500)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    // An overlapping worker already deleting a row is success, not debt.
    for row in &remaining {
        let pending = match entry.collection {
            "sandbox_usage_events" => guards::undelivered(row, options.billing_enabled),
            "sandbox_billing_state" => guards::unsettled_billing(row),
            _ => false,
        };
        if pending {
            if let Some(context) = contexts.iter().find(|context| {
                entry
                    .rules
                    .iter()
                    .any(|rule| matches(entry, rule.erasure, row, context))
            }) {
                return Err(Error::blocked(
                    &context.subject,
                    ErasureBlock::BillingSettlementPending,
                ));
            }
        }
    }
    if !remaining.is_empty() {
        batch.waiting = true;
        batch.has_more = true;
    }
    Ok(())
}
fn count_redaction(name: &str, outcome: Redaction, batch: &mut Batch, counted: bool) {
    match outcome {
        Redaction::Updated if !counted => *batch.redacted.entry(name.into()).or_default() += 1,
        Redaction::Deleted => *batch.deleted.entry(name.into()).or_default() += 1,
        Redaction::Conflict => batch.has_more = true,
        Redaction::Waiting | Redaction::DrainRequired => {
            batch.waiting = true;
            batch.has_more = true;
        }
        _ => {}
    }
}
pub(super) async fn redact(
    db: &Database,
    entry: &Entry,
    row: &Document,
    identity_scope: IdentityScope<'_>,
    now: &str,
    retained: &mut BTreeSet<String>,
    batch: &mut Batch,
) -> Result<bool, Error> {
    let IdentityScope { subjects, agents } = identity_scope;
    let identity = super::identity_refs::redact(db, entry.collection, row, subjects).await?;
    count_redaction(entry.collection, identity, batch, false);
    if matches!(identity, Redaction::Conflict | Redaction::Waiting) {
        return Ok(false);
    }
    let fresh = if identity == Redaction::Updated {
        majority_collection::<Document>(db, entry.collection)
            .find_one(
                doc! {"_id":row.get("_id").cloned().ok_or(Error::InvalidRecord(entry.collection))?},
            )
            .await?
    } else {
        None
    };
    if identity == Redaction::Updated && fresh.is_none() {
        return Ok(false);
    }
    let row = fresh.as_ref().unwrap_or(row);
    let outcome = if matches!(
        entry.collection,
        "agent_profiles"
            | "workspace_profiles"
            | "sandbox_profiles"
            | "agent_profile_revisions"
            | "workspace_profile_revisions"
            | "sandbox_profile_revisions"
    ) {
        super::profile_refs::apply(db, entry.collection, row, subjects, retained, now).await?
    } else {
        super::references::redact_content(db, entry.collection, row, subjects, agents, now).await?
    };
    count_redaction(
        entry.collection,
        outcome,
        batch,
        identity == Redaction::Updated,
    );
    Ok(outcome == Redaction::DrainRequired)
}
