//! Fresh delivery, retirement and exact checked-snapshot fences.
use super::{context::Context, *};
use crate::store::durability::majority_collection;
use mongodb::bson::{doc, Bson, Document};

pub(super) fn unsettled_billing(row: &Document) -> bool {
    row.get("pending")
        .is_some_and(|value| !matches!(value, Bson::Null))
        || row
            .get_array("windows")
            .is_ok_and(|windows| !windows.is_empty())
}
pub(super) fn undelivered(row: &Document, enabled: bool) -> bool {
    row.get_bool("billing_delivered").ok() != Some(true)
        && (enabled
            || row
                .get("billing_claim")
                .is_some_and(|claim| !matches!(claim, Bson::Null)))
}
pub(super) fn usage_filter(enabled: bool) -> Document {
    if enabled {
        doc! {"billing_delivered":true}
    } else {
        doc! {"$or":[{"billing_claim":Bson::Null},{"billing_delivered":true}]}
    }
}
pub(super) async fn billing_page(
    db: &Database,
    contexts: &[Context],
    cursor: &mut Cursor,
) -> Result<(), Error> {
    for context in contexts {
        if majority_collection::<Document>(db,"sandbox_billing_state").find_one(doc! {
            "owner_id":&context.subject,"_id":{"$nin":context.protected.iter().collect::<Vec<_>>()},
            "$or":[{"pending":{"$exists":true,"$ne":Bson::Null}},{"windows.0":{"$exists":true}}]
        }).projection(doc! {"_id":1}).await?.is_some() {
            return Err(Error::blocked(&context.subject, ErasureBlock::BillingSettlementPending));
        }
    }
    cursor.billing_checked = true;
    Ok(())
}
pub(super) async fn preflight(
    db: &Database,
    targets: &[Target<'_>],
    contexts: &[Context],
    now: &str,
    options: Options,
    billing_checked: bool,
    batch: &mut Batch,
) -> Result<(), Error> {
    for context in contexts {
        let target = targets
            .iter()
            .find(|target| target.subject.as_str() == context.subject)
            .ok_or(Error::InvalidRecord("erasure cohort"))?;
        let ids = context.ids(catalog::InventorySet::Sandboxes);
        if !billing_checked && !ids.is_empty() {
            let billing = majority_collection::<Document>(db, "sandbox_billing_state")
                .find_one(doc! {"_id":{"$in":&ids},"$or":[
                    {"pending":{"$exists":true,"$ne":Bson::Null}}, {"windows.0":{"$exists":true}}
                ]})
                .projection(doc! {"_id":1})
                .await?;
            if billing.is_some() {
                return Err(Error::blocked(
                    &context.subject,
                    ErasureBlock::BillingSettlementPending,
                ));
            }
        }
        let mut event_filter = doc! {
            "$or":[{"owner":&context.subject},{"sandbox_id":{"$in":&ids}}],
            "billing_delivered":{"$ne":true},
            "sandbox_id":{"$nin":context.protected.iter().collect::<Vec<_>>()}
        };
        if !options.billing_enabled {
            event_filter.insert("billing_claim", doc! {"$exists":true,"$ne":Bson::Null});
        }
        if !billing_checked
            && majority_collection::<Document>(db, "sandbox_usage_events")
                .find_one(event_filter)
                .await?
                .is_some()
        {
            return Err(Error::blocked(
                &context.subject,
                ErasureBlock::BillingSettlementPending,
            ));
        }
        for row in &context.sandboxes {
            let sandbox = super::super::sandboxes::read_sandbox(row)
                .map_err(|_| Error::InvalidRecord("sandboxes").attribute(&context.subject))?;
            if sandbox.status != layer2_core::sandbox::SandboxStatus::Retired
                || !lease_expired(&sandbox, now)
            {
                batch.waiting = true;
            }
            let keys: BTreeSet<_> = sandbox
                .credential
                .revoke_debt
                .iter()
                .map(|debt| debt.provider_key_id.clone())
                .chain(
                    sandbox
                        .credential
                        .current
                        .iter()
                        .map(|key| key.provider_key_id.clone()),
                )
                .chain(
                    sandbox
                        .credential
                        .pending
                        .iter()
                        .map(|key| key.provider_key_id.clone()),
                )
                .filter(|id| !target.inventory.pending_key_ids.contains(id))
                .collect();
            if !keys.is_empty() {
                batch
                    .pending_key_ids
                    .entry(context.subject.clone())
                    .or_default()
                    .extend(keys);
                batch.waiting = true;
            }
        }
    }
    batch.has_more |= batch.waiting;
    Ok(())
}
fn lease_expired(sandbox: &layer2_core::sandbox::Sandbox, now: &str) -> bool {
    let now = mongodb::bson::DateTime::parse_rfc3339_str(now).ok();
    sandbox.work.lease.as_ref().is_none_or(|lease| matches!((mongodb::bson::DateTime::parse_rfc3339_str(&lease.expires_at).ok(),now),(Some(expiry),Some(now)) if expiry <= now))
}
pub(super) fn sandbox_filter(
    contexts: &[Context],
    ids: &[Bson],
    now: &str,
) -> Result<Document, Error> {
    let mut clauses = vec![];
    for row in contexts
        .iter()
        .flat_map(|context| &context.sandboxes)
        .filter(|row| row.get("_id").is_some_and(|id| ids.contains(id)))
    {
        let sandbox = super::super::sandboxes::read_sandbox(row)
            .map_err(|_| Error::InvalidRecord("sandboxes"))?;
        if sandbox.status != layer2_core::sandbox::SandboxStatus::Retired
            || !lease_expired(&sandbox, now)
        {
            continue;
        }
        // The exact lease that passed the same parsed-instant check above must
        // still be present. String ordering is never a time comparison.
        let lease = super::context::value(row, "record.work.lease")
            .cloned()
            .unwrap_or(Bson::Null);
        clauses.push(doc! {"_id":sandbox.id.as_str(),"owner":sandbox.owner.as_str(),"revision":row.get("revision").cloned().ok_or(Error::InvalidRecord("sandboxes"))?,"record.revision":i64::try_from(sandbox.revision).map_err(|_|Error::InvalidRecord("sandboxes"))?,"status":"retired","record.status":"retired","record.work.lease":{"$eq":lease}});
    }
    Ok(if clauses.is_empty() {
        doc! {"_id":{"$in":[]}}
    } else {
        doc! {"_id":{"$in":ids},"$or":clauses}
    })
}
pub(super) fn billing_filter(rows: &[Document], ids: &[Bson]) -> Document {
    let clauses: Vec<_> = rows
        .iter()
        .filter(|row| row.get("_id").is_some_and(|id| ids.contains(id)))
        .map(|row| {
            let mut filter = doc! {"_id":row.get("_id").unwrap().clone()};
            for field in [
                "revision", "pending", "windows", "current", "stopped", "owner_id",
            ] {
                filter.insert(
                    field,
                    doc! {"$eq":row.get(field).cloned().unwrap_or(Bson::Null)},
                );
            }
            filter
        })
        .collect();
    doc! {"_id":{"$in":ids},"$or":clauses}
}
