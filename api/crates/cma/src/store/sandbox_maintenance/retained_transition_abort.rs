//! Release the exact grant before provider mutation can start.
use super::*;

impl MongoSandboxStore {
    pub(crate) async fn abort_retained_runtime_grant(
        &self,
        database: &Database,
        owner: &layer2_core::SubjectId,
        grant_id: &str,
    ) -> Result<(), SandboxStoreError> {
        let path = grant_path(grant_id)?;
        let original = self.read_retained_runtime_grant(owner, grant_id).await?;
        let mut retries = retry::Retries::new();
        while retries.next().await {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let applied = self
                .abort_retained_runtime_grant_once(database, &path, &original.request, &mut session)
                .await;
            match applied {
                Ok(true) => {}
                Ok(false) => {
                    session.abort_guarded().await.map_err(unavailable)?;
                    return Ok(());
                }
                Err(ApplyError::Driver(error)) if is_confirmed_body_retry(&error) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(public_error(error));
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Failed(error) => return Err(unavailable(error)),
                CommitOutcome::Unknown(_) => {
                    let stored = self.read_retained_runtime_grant(owner, grant_id).await?;
                    if stored.request == original.request && stored.aborted_at.is_some() {
                        return Ok(());
                    }
                    return Err(unresolved());
                }
            }
        }
        Err(unresolved())
    }

    async fn abort_retained_runtime_grant_once(
        &self,
        database: &Database,
        path: &str,
        request: &RetainedRuntimeTransitionRequest,
        session: &mut ClientSession,
    ) -> Result<bool, ApplyError> {
        let grant = self.retained_runtime_grant(path, request, session).await?;
        if grant.aborted_at.is_some() {
            return Ok(false);
        }
        if fence_subject_in_session(database, &request.owner, session)
            .await
            .map_err(ApplyError::Driver)?
        {
            return Err(refused());
        }
        if let Some(receipt) = &grant.receipt {
            let operation = MaintenanceRequest::RetainedTransitionAbort {
                receipt: Box::new(receipt.clone()),
                requested_at: now_rfc3339(),
            };
            let applied = transaction::apply_with_sources(
                self,
                &operation,
                session,
                None,
                IntegrityFailurePolicy::ReportOnly,
            )
            .await?;
            if applied.event.is_none() {
                return Err(refused());
            }
        }
        let updated = self
            .sandbox_admissions
            .update_one(
                doc! {
                    "_id": request.owner.as_str(),
                    (path): bson::serialize_to_bson(&grant).map_err(|_| refused())?,
                },
                doc! {"$currentDate": { (format!("{path}.aborted_at")): true }},
            )
            .session(session)
            .await
            .map_err(ApplyError::Driver)?;
        if updated.modified_count != 1 {
            return Err(refused());
        }
        Ok(true)
    }
}
