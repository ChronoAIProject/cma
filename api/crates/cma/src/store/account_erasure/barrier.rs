//! A majority erasure barrier drains older snapshots before reference discovery.
use super::drain_qualification::{
    invalid as invalid_drain, MAX_DRAIN_SECONDS, MAX_LIFETIME_SECONDS,
};
use super::{Batch, CohortBlock, Cursor, Error, Options, Target, TransactionDrainQualification};
use crate::store::durability::majority_collection;
use mongodb::{
    bson::{doc, DateTime, Document},
    error::ErrorKind,
    Database,
};

const CATALOGUES: &[&str] = &[
    "agent_profiles",
    "workspace_profiles",
    "sandbox_profiles",
    "trigger_definitions",
    "trigger_definition_revisions",
];

pub(super) fn validate(options: Options) -> Result<TransactionDrainQualification, Error> {
    if options.transaction_drain_minimum_secs > MAX_DRAIN_SECONDS {
        return Err(invalid_drain(
            "CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS",
            MAX_DRAIN_SECONDS,
        ));
    }
    options
        .drain_qualification
        .ok_or_else(|| Error::cohort(CohortBlock::TransactionDrainQualificationRequired))
}

async fn observed_lifetime(db: &Database) -> Result<Option<u64>, Error> {
    match db
        .client()
        .database("admin")
        .run_command(doc! {"getParameter":1,"transactionLifetimeLimitSeconds":1})
        .await
    {
        Ok(row) => integer(&row, "transactionLifetimeLimitSeconds")
            .map(Some)
            .ok_or_else(|| invalid_drain("transactionLifetimeLimitSeconds", MAX_LIFETIME_SECONDS)),
        // A qualified bound can support least-privilege credentials. Other
        // command and transport failures do not establish permission absence.
        Err(error) if matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == 13) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn ready(
    db: &Database,
    targets: &[Target<'_>],
    cursor: &mut Cursor,
    options: Options,
    batch: &mut Batch,
) -> Result<bool, Error> {
    let qualification = validate(options)?;
    let seconds = qualification.seconds(
        observed_lifetime(db).await?,
        options.transaction_drain_minimum_secs,
    )?;
    if cursor.drain_complete {
        return Ok(true);
    }
    if cursor.drain_until.is_none() {
        install(db, targets).await?;
        // Start the qualified window after every fence is majority-acknowledged.
        // Its external qualification must include clock uncertainty.
        let server = db.run_command(doc! {"hello":1}).await?;
        let now = *server
            .get_datetime("localTime")
            .map_err(|_| Error::Store("server clock is unavailable".into()))?;
        #[cfg(test)]
        let seconds = cursor.test_drain_seconds.unwrap_or(seconds);
        let millis = i64::try_from(seconds)
            .ok()
            .and_then(|value| value.checked_mul(1000))
            .and_then(|value| now.timestamp_millis().checked_add(value))
            .ok_or_else(|| Error::Store("transaction drain deadline is out of range".into()))?;
        cursor.drain_until = Some(DateTime::from_millis(millis));
    }
    let server = db.run_command(doc! {"hello":1}).await?;
    let now = server
        .get_datetime("localTime")
        .map_err(|_| Error::Store("server clock is unavailable".into()))?;
    if cursor.drain_until.is_some_and(|deadline| *now < deadline) {
        batch.waiting = true;
        batch.has_more = true;
        return Ok(false);
    }
    if install(db, targets).await? > 0 {
        cursor.drain_until = None;
        batch.waiting = true;
        batch.has_more = true;
        return Ok(false);
    }
    cursor.drain_complete = true;
    Ok(true)
}
fn integer(row: &Document, field: &str) -> Option<u64> {
    row.get_i64(field)
        .or_else(|_| row.get_i32(field).map(i64::from))
        .ok()
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(value).ok())
}

async fn install(db: &Database, targets: &[Target<'_>]) -> Result<u64, Error> {
    let owners: Vec<_> = targets
        .iter()
        .map(|target| target.subject.as_str())
        .collect();
    let mut modified = 0;
    for name in CATALOGUES {
        modified += majority_collection::<Document>(db,name).update_many(doc! {
            "owner":{"$in":&owners},"$or":[{"account_erasure_fenced":{"$ne":true}},
            {"account_erasure_fenced_at":{"$not":{"$type":"date"}}}]
        }, doc! {"$set":{"account_erasure_fenced":true},"$currentDate":{"account_erasure_fenced_at":true}}).await?.modified_count;
    }
    Ok(modified)
}
