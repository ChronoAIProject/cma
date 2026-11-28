use std::time::SystemTime;

use cma_am_auth::auth::service_authorization::RetirementWake;
use futures_util::TryStreamExt;
use layer2_core::{
    rfc3339_at,
    sandbox::{RetirementCredentialRecovery, Sandbox, SandboxEventKind, SandboxStoreError},
    StoreError, SubjectId,
};
use mongodb::bson::{doc, Document};

use super::{
    sandbox_events::read_event, sandbox_mongo::record_bson, sandboxes::read_sandbox,
    MongoSandboxStore,
};

#[async_trait::async_trait]
impl RetirementWake for MongoSandboxStore {
    async fn wake_retirement(&self, owner: &SubjectId) -> Result<(), StoreError> {
        self.wake_retiring_sandboxes(owner, &rfc3339_at(SystemTime::now()))
            .await
            .map_err(|_| StoreError::Unavailable("retirement wake could not be recorded".into()))
    }
}

impl MongoSandboxStore {
    pub(in crate::store) async fn wake_retiring_sandboxes(
        &self,
        owner: &SubjectId,
        now: &str,
    ) -> Result<(), SandboxStoreError> {
        let mut cursor = self
            .sandboxes
            .find(super::sandbox_quarantine::exclude(doc! {
                "owner": owner.as_str(),
                "status": "retiring",
                "work_pending": true,
                "$or": [
                    {"record.work.retirement_credential_recovery": {"$exists": true},
                     "record.work.retirement_credential_recovery.admitted": null},
                    {"work_ready_at": "9999-12-31T23:59:59Z"},
                ],
            }))
            .await
            .map_err(unavailable)?;
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            self.wake_retirement_row(row, owner, now).await?;
        }
        Ok(())
    }

    async fn wake_retirement_row(
        &self,
        mut row: Document,
        owner: &SubjectId,
        now: &str,
    ) -> Result<(), SandboxStoreError> {
        for _ in 0..3 {
            let Ok(sandbox) = read_sandbox(&row) else {
                tracing::warn!("Retirement wake skipped an unreadable Sandbox");
                return Ok(());
            };
            if &sandbox.owner != owner
                || !sandbox.allows_retirement_credential_recovery()
                || sandbox.retirement_archive_pending()
            {
                return Ok(());
            }
            let recovery = match sandbox.work.retirement_credential_recovery {
                Some(recovery) if recovery.admitted.is_none() => recovery,
                None if self.legacy_credential_stop(&row, &sandbox).await? => {
                    RetirementCredentialRecovery {
                        newer_than_ms: 0,
                        expected_scope: [0; 32],
                        previous_token_generation: None,
                        authorization_updated: false,
                        admitted: None,
                    }
                }
                _ => return Ok(()),
            };
            let recovery = RetirementCredentialRecovery {
                authorization_updated: true,
                ..recovery
            };
            let revision = row
                .get_i64("revision")
                .map_err(|_| SandboxStoreError::Conflict)?;
            let next = revision.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            let outcome = self
                .sandboxes
                .update_one(
                    super::sandbox_quarantine::exclude(doc! {
                        "_id": sandbox.id.as_str(), "owner": owner.as_str(), "status": "retiring",
                        "revision": revision,
                    }),
                    doc! { "$set": {
                        "revision": next, "record.revision": next,
                        "work_ready_at": now, "record.work.ready_at": now,
                        "record.work.retirement_credential_recovery": record_bson(&recovery)
                            .map_err(SandboxStoreError::Corrupt)?,
                    }},
                )
                .await
                .map_err(unavailable)?;
            if outcome.modified_count == 1 {
                self.sandbox_admitted.notify_one();
                return Ok(());
            }
            let Some(current) = self
                .sandboxes
                .find_one(doc! {"_id": sandbox.id.as_str(), "owner": owner.as_str()})
                .await
                .map_err(unavailable)?
            else {
                return Ok(());
            };
            row = current;
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn legacy_credential_stop(
        &self,
        row: &Document,
        sandbox: &Sandbox,
    ) -> Result<bool, SandboxStoreError> {
        if !sandbox.retirement_retry_exhausted() {
            return Ok(false);
        }
        let last = row
            .get_i64("last_seq")
            .map_err(|_| SandboxStoreError::Conflict)?;
        let event = self
            .sandbox_events
            .find_one(doc! {
                "sandbox_id": sandbox.id.as_str(), "seq": last,
            })
            .await
            .map_err(unavailable)?;
        let event = event.as_ref().map(read_event).transpose()?;
        Ok(event.is_some_and(|event| {
            matches!(event.kind,
            SandboxEventKind::WorkStepFailed { step, failure }
                if step == sandbox.work.step
                    && (failure.class == "owner_reauthorization_required"
                        || (failure.class == "terminal_security"
                            && failure.detail == "credential authority was refused")))
        }))
    }
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
