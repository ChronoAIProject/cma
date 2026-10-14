use super::{document, model::Record, read, unavailable, NativePreparedPlan};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, record_bson, start_transaction,
    CommitOutcome, MongoSandboxStore,
};
use futures_util::TryStreamExt;
use layer2_core::sandbox::{SandboxStoreError, WarmSandbox, WarmSandboxState};
use mongodb::bson::{doc, Bson};

const MAX_TARGET: u8 = 2;
const MAX_AGE_SECONDS: u64 = 1800;

impl MongoSandboxStore {
    pub(crate) async fn reconcile_native_prepared_target(
        &self,
        desired: Option<&NativePreparedPlan>,
        target: u8,
        at: &str,
    ) -> Result<(), SandboxStoreError> {
        if target > MAX_TARGET
            || (target > 0 && desired.is_none())
            || layer2_core::seconds_from_rfc3339(at).is_none()
        {
            return Err(SandboxStoreError::Conflict);
        }
        if let Some(plan) = desired {
            plan.validate()?;
        }
        for _ in 0..16 {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            match plan(self, desired, target, at, &mut session).await {
                Ok(()) => (),
                Err(PlanError::Driver(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let _ = session.abort_transaction().await;
                    tokio::task::yield_now().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(match error {
                        PlanError::Domain(e) => e,
                        PlanError::Driver(e) => unavailable(e),
                    });
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(e) | CommitOutcome::Failed(e) => return Err(unavailable(e)),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

enum PlanError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}
impl From<SandboxStoreError> for PlanError {
    fn from(e: SandboxStoreError) -> Self {
        Self::Domain(e)
    }
}
impl From<mongodb::error::Error> for PlanError {
    fn from(e: mongodb::error::Error) -> Self {
        Self::Driver(e)
    }
}

async fn plan(
    store: &MongoSandboxStore,
    desired: Option<&NativePreparedPlan>,
    target: u8,
    at: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(), PlanError> {
    // A single fence serializes target planning and ownership transfer across API replicas.
    store.native_prepared.update_one(doc!{"_id":"target"},doc!{"$inc":{"fence":1_i64},"$set":{
        "kind":"target","target":i32::from(target),"plan":record_bson(&desired).map_err(SandboxStoreError::Corrupt)?,
        "document_schema_version":super::DOCUMENT_SCHEMA_VERSION,
    }}).upsert(true).session(&mut *session).await?;
    let mut cursor=store.native_prepared.find(doc!{"kind":"reservation","state":{"$in":["preparing","installing","ready","retiring"]}})
        .sort(doc!{"_id":1}).session(&mut *session).await?;
    let records = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
    let occupied = records.len();
    let mut retained = 0usize;
    let now = layer2_core::seconds_from_rfc3339(at).ok_or(SandboxStoreError::Conflict)?;
    for old in records {
        let mut record = read(&old)?;
        if record.payload.state == WarmSandboxState::Retiring {
            continue;
        }
        let created = layer2_core::seconds_from_rfc3339(&record.payload.created_at)
            .ok_or(SandboxStoreError::Conflict)?;
        if desired == Some(&record.plan)
            && retained < usize::from(target)
            && now.saturating_sub(created) < MAX_AGE_SECONDS
        {
            retained += 1;
            continue;
        }
        record.payload.state = WarmSandboxState::Retiring;
        record.payload.revision = record
            .payload
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        record.payload.updated_at = at.into();
        let mut next = document(&record)?;
        for key in ["lease_token", "lease_until"] {
            next.insert(key, old.get(key).cloned().unwrap_or(Bson::Null));
        }
        next.insert("check_after", at);
        store
            .native_prepared
            .replace_one(doc! {"_id":record.payload.id.as_str()}, next)
            .session(&mut *session)
            .await?;
    }
    if let Some(plan) = desired {
        for _ in 0..usize::from(target)
            .saturating_sub(retained)
            .min(usize::from(MAX_TARGET).saturating_sub(occupied))
        {
            let record = Record {
                plan: plan.clone(),
                payload: WarmSandbox::new(plan.warm_plan().clone(), at.into())?,
                ownership: None,
            };
            let mut value = document(&record)?;
            value.insert("lease_token", Bson::Null);
            value.insert("lease_until", at);
            value.insert("check_after", at);
            store
                .native_prepared
                .insert_one(value)
                .session(&mut *session)
                .await?;
        }
    }
    Ok(())
}
