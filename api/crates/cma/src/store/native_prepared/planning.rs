use super::{document, model::Record, read, unavailable, NativePreparedPlan};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, record_bson, start_transaction,
    CommitOutcome, MongoSandboxStore,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::released_runtime::ReleasedLane;
use layer2_core::sandbox::{SandboxStoreError, WarmSandbox, WarmSandboxState};
use layer3_adapter::ReleasedRuntime;
use mongodb::bson::{doc, Bson};

const MAX_TARGET: u8 = 2;
pub(super) const MAX_AGE_SECONDS: u64 = 1800;

impl MongoSandboxStore {
    #[cfg(test)]
    pub(crate) async fn reconcile_native_prepared_target(
        &self,
        desired: Option<&NativePreparedPlan>,
        target: u8,
        at: &str,
    ) -> Result<(), SandboxStoreError> {
        self.reconcile_native_prepared_class(desired, target, at, false)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn reconcile_native_prepared_class(
        &self,
        desired: Option<&NativePreparedPlan>,
        target: u8,
        at: &str,
        machine: bool,
    ) -> Result<(), SandboxStoreError> {
        let release = self.released_runtime();
        self.reconcile_native_prepared_release(desired, target, at, machine, &release)
            .await
    }

    pub(crate) async fn reconcile_native_prepared_release(
        &self,
        desired: Option<&NativePreparedPlan>,
        target: u8,
        at: &str,
        machine: bool,
        release: &ReleasedRuntime,
    ) -> Result<(), SandboxStoreError> {
        if self.default_pool_holds.lane(machine) {
            return Ok(());
        }
        if target > MAX_TARGET
            || (target > 0 && desired.is_none())
            || layer2_core::seconds_from_rfc3339(at).is_none()
        {
            return Err(SandboxStoreError::Conflict);
        }
        if let Some(plan) = desired {
            plan.validate()?;
            let image = if machine {
                release.machine.as_deref()
            } else {
                Some(release.agent.as_str())
            };
            if plan.machine() != machine
                || (self.released_lanes.is_some()
                    && image != Some(plan.warm.runtime_version.as_str()))
            {
                return Err(SandboxStoreError::Conflict);
            }
        }
        for _ in 0..16 {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            match plan(self, desired, target, at, machine, release, &mut session).await {
                Ok(()) => (),
                Err(PlanError::Driver(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let _ = session.abort_guarded().await;
                    tokio::task::yield_now().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
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
    machine: bool,
    release: &ReleasedRuntime,
    session: &mut mongodb::ClientSession,
) -> Result<(), PlanError> {
    let target_id = if machine { "machine-target" } else { "target" };
    let target_filter = doc! {"_id": target_id};
    let plan_value = record_bson(&desired).map_err(SandboxStoreError::Corrupt)?;
    let previous = store
        .native_prepared
        .find_one(target_filter.clone())
        .session(&mut *session)
        .await?;
    let target_changed = previous.as_ref().is_none_or(|previous| {
        previous.get_i32("target").ok() != Some(i32::from(target))
            || previous.get("plan") != Some(&plan_value)
    });
    if previous.is_none()
        && target == 0
        && desired.is_none()
        && machine
        && release.machine.is_none()
    {
        return Ok(());
    }
    let channel = if machine {
        Bson::Boolean(true)
    } else {
        doc! {"$ne":true}.into()
    };
    let mut cursor=store.native_prepared.find(doc!{"kind":"reservation","machine":channel,"state":{"$in":["preparing","installing","ready","retiring"]}})
        .sort(doc!{"_id":1}).session(&mut *session).await?;
    let records = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
    let occupied = records.len();
    let mut retained = 0usize;
    let mut retiring = Vec::new();
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
        retiring.push((record.payload.id, next));
    }
    let refill = usize::from(target)
        .saturating_sub(retained)
        .min(usize::from(MAX_TARGET).saturating_sub(occupied));
    if !target_changed && retiring.is_empty() && refill == 0 {
        return Ok(());
    }
    // The class write serializes with ownership transfer. The lane write also
    // defeats an old planner whose class target still matches after promotion.
    let lane = if machine {
        ReleasedLane::Machine
    } else {
        ReleasedLane::Agent
    };
    store
        .fence_released_admission(release, lane, &mut *session)
        .await?;
    store
        .native_prepared
        .update_one(
            target_filter,
            doc! {
                "$inc": {"fence": 1_i64},
                "$set": {
                    "kind": "target", "target": i32::from(target), "plan": plan_value,
                    "document_schema_version": super::DOCUMENT_SCHEMA_VERSION,
                },
            },
        )
        .upsert(true)
        .session(&mut *session)
        .await?;
    for (id, next) in retiring {
        store
            .native_prepared
            .replace_one(doc! {"_id": id.as_str()}, next)
            .session(&mut *session)
            .await?;
    }
    if let Some(plan) = desired {
        for _ in 0..refill {
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
