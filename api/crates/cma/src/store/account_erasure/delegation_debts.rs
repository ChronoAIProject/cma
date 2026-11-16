//! Capture old delegation references before retained Bot attribution is erased.
//! The caller persists exact tuples; this adapter never revokes remote grants.
use super::{Batch, Cursor, Error, Target};
use crate::store::durability::majority_collection;
use futures_util::TryStreamExt;
use layer2_core::{
    account_deletion::DelegationDebt, bots::BotOwnership, resource_access::ResourceOwner,
};
use mongodb::{
    bson::{self, doc, Document},
    options::Hint,
    Database,
};
use std::collections::BTreeMap;

const LIMIT: usize = 20_000;
const BYTES: usize = 4 * 1024 * 1024;

fn bytes(debt: &DelegationDebt) -> usize {
    debt.bot_id
        .len()
        .saturating_add(debt.authorization_grant_id.len())
        .saturating_add(debt.credential_incarnation.len())
}

/// Read at most 500 projected source rows per call. A page with missing durable
/// acknowledgements is replayed unchanged; mutations start after the full scan.
pub(super) async fn page(
    db: &Database,
    targets: &[Target<'_>],
    cursor: &mut Cursor,
    batch: &mut Batch,
) -> Result<bool, Error> {
    if cursor.delegations_checked {
        return Ok(true);
    }
    for target in targets {
        let (count, size) = target
            .inventory
            .pending_delegations
            .iter()
            .fold((0_usize, 0_usize), |(count, size), debt| {
                (count + 1, size.saturating_add(bytes(debt)))
            });
        if count > LIMIT || size > BYTES {
            return Err(Error::capacity(
                target.subject.as_str(),
                "delegation inventory",
                LIMIT,
            ));
        }
    }
    let filter = cursor.delegations_after.as_ref().map_or_else(
        Document::new,
        |after| doc! {"$expr":{"$gt":["$_id",{"$literal":after}]}},
    );
    let mut rows = majority_collection::<Document>(db, "bots").find(filter)
        .projection(doc! {"_id":1,"document_schema_version":1,"owner":1,"bot.bot_id":1,"bot.owner":1,"bot.ownership":1})
        .sort(doc! {"_id":1}).hint(Hint::Name("_id_".into())).limit(500).batch_size(32).await?;
    let mut page_counts = BTreeMap::<String, usize>::new();
    let mut page_bytes = BTreeMap::<String, usize>::new();
    let mut missing = false;
    let mut length = 0_usize;
    let mut last = None;
    while let Some(row) = rows.try_next().await? {
        length += 1;
        last = row.get("_id").cloned();
        let Some(subject) = super::context::value(&row, "bot.ownership.credential_subject")
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let Some(target) = targets
            .iter()
            .find(|target| target.subject.as_str() == subject)
        else {
            continue;
        };
        let debt = decode(&row).map_err(|error| error.attribute(subject))?;
        let Some(debt) = debt else {
            continue;
        };
        let count = page_counts.entry(subject.to_owned()).or_default();
        *count = count.saturating_add(1);
        let size = page_bytes.entry(subject.to_owned()).or_default();
        *size = size.saturating_add(bytes(&debt));
        if cursor
            .delegation_counts
            .get(subject)
            .copied()
            .unwrap_or_default()
            .saturating_add(*count)
            > LIMIT
            || cursor
                .delegation_bytes_by_subject
                .get(subject)
                .copied()
                .unwrap_or_default()
                .saturating_add(*size)
                > BYTES
        {
            return Err(Error::capacity(subject, "delegation scan", LIMIT));
        }
        if !target.inventory.pending_delegations.contains(&debt) {
            batch
                .pending_delegations
                .entry(subject.to_owned())
                .or_default()
                .push(debt);
            missing = true;
        }
    }
    if missing {
        batch.has_more = true;
        batch.waiting = true;
        return Ok(false);
    }
    // Commit this page's independent totals only after every captured tuple
    // appears in durable inventory. Replaying unacknowledged pages is free.
    for (subject, count) in page_counts {
        let total = cursor.delegation_counts.entry(subject).or_default();
        *total = total.saturating_add(count);
    }
    for (subject, size) in page_bytes {
        let total = cursor
            .delegation_bytes_by_subject
            .entry(subject)
            .or_default();
        *total = total.saturating_add(size);
    }
    cursor.delegations_after = last;
    cursor.delegations_checked = length < 500;
    batch.has_more |= !cursor.delegations_checked;
    Ok(cursor.delegations_checked)
}

fn decode(row: &Document) -> Result<Option<DelegationDebt>, Error> {
    const NAME: &str = "bots";
    crate::store::sandbox_mongo::require_version(row).map_err(|_| Error::InvalidRecord(NAME))?;
    let value = super::context::value(row, "bot.ownership")
        .cloned()
        .ok_or(Error::InvalidRecord(NAME))?;
    let ownership: BotOwnership =
        bson::deserialize_from_bson(value).map_err(|_| Error::InvalidRecord(NAME))?;
    ownership
        .validate()
        .map_err(|_| Error::InvalidRecord(NAME))?;
    let owner = match &ownership.owner {
        ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => {
            subject.as_str()
        }
    };
    let id = row.get_str("_id").map_err(|_| Error::InvalidRecord(NAME))?;
    if row.get_str("owner").ok() != Some(owner)
        || super::context::value(row, "bot.owner").and_then(|value| value.as_str()) != Some(owner)
        || super::context::value(row, "bot.bot_id").and_then(|value| value.as_str()) != Some(id)
    {
        return Err(Error::InvalidRecord(NAME));
    }
    match (
        ownership.authorization_grant_id,
        ownership.credential_incarnation,
    ) {
        (Some(authorization_grant_id), Some(credential_incarnation)) => Ok(Some(DelegationDebt {
            bot_id: id.to_owned(),
            authorization_grant_id,
            credential_incarnation,
        })),
        (None, None) => Ok(None),
        _ => Err(Error::InvalidRecord(NAME)),
    }
}
