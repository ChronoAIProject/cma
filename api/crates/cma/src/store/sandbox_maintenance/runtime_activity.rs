//! Read-only coherent source capture, separate from admission's write fences.
use layer2_core::sandbox::store::runtime_activity::*;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::SandboxId;
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;
use std::time::Duration;

use super::transaction::ApplyError;
use crate::store::sandbox_mongo::MongoSandboxStore;

mod admission;
mod capture;
mod projection;
mod records;
mod storage;
pub(super) use admission::validate_for_admission;

const CAPTURE_BUDGET: Duration = Duration::from_secs(5);

#[async_trait::async_trait]
impl SandboxRuntimeActivityObservationStore for MongoSandboxStore {
    async fn observe_runtime_activity(
        &self,
        id: &SandboxId,
        context: &RuntimeActivitySourceContext,
    ) -> Result<RuntimeActivityObservation, SandboxStoreError> {
        if context.coverage != RuntimeActivityCoverage::MonotonicWritersV1
            || context.validate().is_err()
        {
            return Ok(RuntimeActivityObservation::Unknown(
                RuntimeActivityUnknownReason::UnqualifiedSource,
            ));
        }
        if context.process_incarnation
            != layer2_core::sandbox::runtime_desktop_process_incarnation()
            || context.database_id != self.sandboxes.namespace().db
            || self.agent_ownership.is_some()
            || self.renewable_work
        {
            return Ok(RuntimeActivityObservation::Unknown(
                RuntimeActivityUnknownReason::UnsupportedContext,
            ));
        }
        let result = tokio::time::timeout(CAPTURE_BUDGET, async {
            // Snapshot sessions do not call our transaction helper, which can
            // write an ordinary-work fence. This observer performs only reads.
            let mut session = self
                .client
                .start_session()
                .snapshot(true)
                .await
                .map_err(ApplyError::Driver)?;
            capture::snapshot(self, id, context, &mut session, true).await
        })
        .await;
        match result {
            Ok(Ok(snapshot)) => Ok(RuntimeActivityObservation::Snapshot(Box::new(snapshot))),
            Ok(Err(ApplyError::Domain(SandboxStoreError::NotFound))) => {
                Err(SandboxStoreError::NotFound)
            }
            Ok(Err(ApplyError::Domain(_))) => Ok(RuntimeActivityObservation::Unknown(
                RuntimeActivityUnknownReason::IncompleteCoverage,
            )),
            Ok(Err(ApplyError::Driver(_))) | Err(_) => {
                tracing::warn!(
                    "Runtime activity observation unavailable; quiet coverage remains unknown"
                );
                Ok(RuntimeActivityObservation::Unknown(
                    RuntimeActivityUnknownReason::Unavailable,
                ))
            }
        }
    }
}

fn invalid() -> ApplyError {
    ApplyError::Domain(SandboxStoreError::InvalidTransaction(
        "runtime activity source is incomplete or changed",
    ))
}

fn now_ms() -> Result<i64, ApplyError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or_else(invalid)
}

fn desktop(id: &SandboxId) -> Result<RuntimeActivityDesktop, ApplyError> {
    let value =
        layer2_core::sandbox::runtime_desktop_activity_snapshot(id).map_err(|_| invalid())?;
    Ok(RuntimeActivityDesktop {
        process_incarnation: value.process_incarnation,
        tracking_incarnation: value.tracking_incarnation,
        activity_epoch: value.activity_epoch,
        opening_or_open: value.opening_or_open,
    })
}

async fn bounded_find(
    collection: &mongodb::Collection<Document>,
    filter: Document,
    projection: Document,
    session: &mut ClientSession,
    snapshot_read: bool,
) -> Result<Vec<Document>, ApplyError> {
    use futures_util::TryStreamExt;
    let mut find = collection
        .find(filter)
        .projection(projection)
        .sort(doc! {"_id":1})
        .limit((RUNTIME_ACTIVITY_MAX_ROWS + 1) as i64)
        .max_time(Duration::from_secs(2));
    if snapshot_read {
        find = find.read_concern(mongodb::options::ReadConcern::snapshot());
    }
    let mut cursor = find
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut rows = Vec::new();
    while let Some(row) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(ApplyError::Driver)?
    {
        if rows.len() == RUNTIME_ACTIVITY_MAX_ROWS {
            return Err(invalid());
        }
        // Bound projected bytes as well as rows. Payload fields never enter the
        // projection; a corrupt oversized metadata value remains a refusal.
        if mongodb::bson::serialize_to_vec(&row)
            .map_err(|_| invalid())?
            .len()
            > 32_768
        {
            return Err(invalid());
        }
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
mod tests;
