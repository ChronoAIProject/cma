//! A majority erasure barrier drains older snapshots before reference discovery.
use super::{Batch, CohortBlock, Cursor, Error, Options, Target};
use crate::store::durability::majority_collection;
use mongodb::{
    bson::{doc, DateTime, Document},
    Database,
};

const MAX_LIFETIME_SECONDS: u64 = 3_600;
const MAX_DRAIN_SECONDS: u64 = 86_400;

fn invalid_drain(parameter: &'static str, maximum_seconds: u64) -> Error {
    Error::cohort(CohortBlock::InvalidTransactionDrain {
        parameter,
        maximum_seconds,
    })
}

const CATALOGUES: &[&str] = &[
    "agent_profiles",
    "workspace_profiles",
    "sandbox_profiles",
    "trigger_definitions",
    "trigger_definition_revisions",
];

pub(super) async fn ready(
    db: &Database,
    targets: &[Target<'_>],
    cursor: &mut Cursor,
    options: Options,
    batch: &mut Batch,
) -> Result<bool, Error> {
    if options.transaction_drain_fallback_secs > MAX_DRAIN_SECONDS {
        return Err(invalid_drain(
            "CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS",
            MAX_DRAIN_SECONDS,
        ));
    }
    if cursor.drain_complete {
        return Ok(true);
    }
    if cursor.drain_until.is_none() {
        install(db, targets).await?;
        // Read the server clock *after* every majority acknowledgement. This
        // deadline is safe across worker restarts, hosts and local clock skew.
        let server = db.run_command(doc! {"hello":1}).await?;
        let now = *server
            .get_datetime("localTime")
            .map_err(|_| Error::Store("server clock is unavailable".into()))?;
        let parameter = db
            .client()
            .database("admin")
            .run_command(doc! {"getParameter":1,"transactionLifetimeLimitSeconds":1})
            .await;
        let lifetime =
            match parameter {
                Ok(row) => Some(integer(&row, "transactionLifetimeLimitSeconds").ok_or_else(
                    || invalid_drain("transactionLifetimeLimitSeconds", MAX_LIFETIME_SECONDS),
                )?),
                Err(_) => None,
            };
        let seconds = drain_seconds(lifetime, options.transaction_drain_fallback_secs)?;
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

fn drain_seconds(lifetime: Option<u64>, fallback: u64) -> Result<u64, Error> {
    if lifetime.is_some_and(|seconds| seconds == 0 || seconds > MAX_LIFETIME_SECONDS) {
        return Err(invalid_drain(
            "transactionLifetimeLimitSeconds",
            MAX_LIFETIME_SECONDS,
        ));
    }
    if fallback > MAX_DRAIN_SECONDS {
        return Err(invalid_drain(
            "CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS",
            MAX_DRAIN_SECONDS,
        ));
    }
    // Expired transactions can survive until the periodic cleanup task runs.
    // Ten extra seconds cover cleanup scheduling and acknowledgement jitter;
    // the operator's configured floor always applies, including readable hosts.
    Ok(lifetime
        .map(|seconds| {
            seconds
                .saturating_add(seconds.div_ceil(2))
                .saturating_add(10)
                .max(fallback)
        })
        .unwrap_or(fallback.max(120)))
}
#[cfg(test)]
mod tests {
    #[test]
    fn cleanup_lifetime_and_configured_floor_both_apply() {
        assert_eq!(super::drain_seconds(Some(60), 0).unwrap(), 100);
        assert_eq!(super::drain_seconds(Some(61), 0).unwrap(), 102);
        assert_eq!(super::drain_seconds(Some(60), 120).unwrap(), 120);
        assert_eq!(super::drain_seconds(Some(300), 120).unwrap(), 460);
        assert_eq!(super::drain_seconds(Some(60), 240).unwrap(), 240);
        assert_eq!(super::drain_seconds(None, 120).unwrap(), 120);
        assert_eq!(super::drain_seconds(None, 240).unwrap(), 240);
        assert_eq!(super::drain_seconds(None, 0).unwrap(), 120);
        assert!(matches!(
            super::drain_seconds(Some(u64::MAX), 120),
            Err(super::Error::CohortBlocked(_))
        ));
        assert!(super::drain_seconds(Some(0), 120).is_err());
        assert!(super::drain_seconds(Some(3600), 86400).is_ok());
        assert!(super::drain_seconds(Some(3601), 120).is_err());
        assert!(super::drain_seconds(None, u64::MAX).is_err());
    }
}
