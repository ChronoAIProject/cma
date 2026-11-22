//! Publish fixed retention under the capture schedule's existing fenced write.
use super::*;
use layer2_core::sandbox::home_backup::retention::{retain_ready_points, ReadyPoint};
use mongodb::ClientSession;

// A thirty-day capture window normally has 1,440 points. Bound exceptional
// histories before returning a policy; never apply a partial classification.
const MAX_HISTORY: usize = 4096;

pub(super) async fn publish(
    store: &MongoHomeBackupStore,
    incoming: &mut HomeBackup,
    session: &mut ClientSession,
) -> Result<(), HomeBackupError> {
    let ready_at = incoming
        .cut
        .ready_at
        .as_deref()
        .ok_or(HomeBackupError::Invalid("ready point lacks time"))?;
    let seconds = layer2_core::seconds_from_rfc3339(ready_at)
        .ok_or(HomeBackupError::Invalid("invalid ready time"))?;
    // Include the whole first Singapore day so an expired/deleted point still
    // prevents a later ready point from becoming the first one in that day.
    let earliest = seconds.saturating_sub(30 * 86400);
    let cutoff = ((earliest + 8 * 3600) / 86400 * 86400).saturating_sub(8 * 3600);
    let cutoff =
        layer2_core::rfc3339_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(cutoff));
    let mut cursor = store
        .backups
        .find(doc! {
            "sandbox_id":&incoming.sandbox_id,
            // A replacement home starts a new history. Runtime generations and
            // boots can change while this exact home/source pair stays stable.
            "source.home_identity":&incoming.source.home_identity,
            "source.pvc_uid":&incoming.source.home_source_id,
            "cut.ready_at":{"$gte":cutoff},
            "_id":{"$ne":&incoming.id}
        })
        .limit((MAX_HISTORY + 1) as i64)
        .session(&mut *session)
        .await
        .map_err(failed)?;
    let mut history = Vec::new();
    while let Some(row) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(failed)?
    {
        if history.len() == MAX_HISTORY {
            return Err(HomeBackupError::Invalid(
                "ready history exceeds publication bound",
            ));
        }
        history.push(read_backup(row).map_err(malformed)?);
    }
    let points: Vec<_> = history
        .iter()
        .chain(std::iter::once(&*incoming))
        .map(|value| {
            Ok(ReadyPoint {
                backup_id: value.id.clone(),
                ready_at: value
                    .cut
                    .ready_at
                    .clone()
                    .ok_or(HomeBackupError::Invalid("ready history lacks time"))?,
            })
        })
        .collect::<Result<_, HomeBackupError>>()?;
    for decision in retain_ready_points(&points)? {
        if decision.backup_id == incoming.id {
            incoming.retention = decision.retention;
            continue;
        }
        let previous = history
            .iter()
            .find(|row| row.id == decision.backup_id)
            .ok_or(HomeBackupError::Conflict)?;
        if previous.state != HomeBackupState::Ready || previous.retention == decision.retention {
            continue;
        }
        let mut updated = previous.clone();
        updated.retention = decision.retention;
        updated.revision = increment(previous.revision)?;
        let result = store
            .backups
            .replace_one(
                doc! {
                    "_id":&previous.id,"state":"ready","lease":mongodb::bson::Bson::Null,
                    "revision":codec::signed(previous.revision).map_err(malformed)?
                },
                backup_document(&updated).map_err(malformed)?,
            )
            .session(&mut *session)
            .await
            .map_err(failed)?;
        if result.matched_count != 1 {
            return Err(HomeBackupError::Conflict);
        }
    }
    incoming.validate()
}
