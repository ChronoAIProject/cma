//! The durable deletion inventory can explain a missing publisher revision.
use super::{records, RECORDS};
use futures_util::TryStreamExt;
use layer2_core::{account_deletion::ErasureStep, trigger_domain::TriggerId};
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(in crate::store) enum Error {
    Driver(mongodb::error::Error),
    Corrupt,
}

pub(in crate::store) async fn prove(db: &Database, id: &TriggerId) -> Result<bool, Error> {
    // Completed records scrub their inventories. The active predicate also
    // permits the existing partial index without requiring a collection that
    // a legacy store may not have initialized yet.
    let rows = crate::store::durability::majority_collection::<Document>(db, RECORDS)
        .find(doc! {"active_subject_hash":{"$exists":true},"inventory.triggers":id.as_str()})
        .limit(2)
        .await
        .map_err(Error::Driver)?
        .try_collect::<Vec<_>>()
        .await
        .map_err(Error::Driver)?;
    if rows.len() > 1 {
        return Err(Error::Corrupt);
    }
    let Some(row) = rows.into_iter().next() else {
        return Ok(false);
    };
    records::request_keys(&row).map_err(|_| Error::Corrupt)?;
    let record = records::read_record(row).map_err(|_| Error::Corrupt)?;
    if !record.is_active()
        || !record
            .inventory
            .triggers
            .iter()
            .any(|value| value == id.as_str())
    {
        return Err(Error::Corrupt);
    }
    Ok(matches!(
        record.step,
        ErasureStep::EraseRecords
            | ErasureStep::EraseObjects
            | ErasureStep::RevokeCredentials
            | ErasureStep::VerifyNoResidue
    ))
}
