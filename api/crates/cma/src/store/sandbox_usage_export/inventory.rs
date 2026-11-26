use super::super::sandbox_events::TransactionEventError;
use super::super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::ClientSession;

impl CaptureStore {
    pub(super) async fn baseline(
        &self,
        store: &MongoSandboxStore,
        at: &str,
        limit: usize,
    ) -> Result<(), SandboxStoreError> {
        if !(1..=32).contains(&limit) {
            return Err(corrupt());
        }
        for attempt in 0..TRANSACTION_ATTEMPTS {
            retry_delay(attempt).await;
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            if let Err(error) = self.baseline_in(store, &mut session, at, limit).await {
                let _ = session.abort_guarded().await;
                match error {
                    TransactionEventError::Driver(error)
                        if is_transient(&error) || is_duplicate_key(&error) =>
                    {
                        continue
                    }
                    TransactionEventError::Driver(error) => return Err(unavailable(error)),
                    TransactionEventError::Domain(error) => return Err(error),
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error));
                }
            }
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn baseline_in(
        &self,
        store: &MongoSandboxStore,
        session: &mut ClientSession,
        at: &str,
        limit: usize,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let rows = self.collection(INVENTORY);
        let saved = rows
            .find_one(doc! {"_id":&self.config.namespace})
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        let mut scan = saved.clone().unwrap_or_else(|| {
            doc! {
                "_id":&self.config.namespace,"document_schema_version":1_i64,
                "owner_issuer":&self.config.owner_issuer,"started_at":at,
                "after":"","registered":0_i64,"owner_kind_unresolved":0_i64,"complete":false,
            }
        });
        if scan.get_i64("document_schema_version").ok() != Some(1)
            || scan.get_str("owner_issuer").ok() != Some(&self.config.owner_issuer)
            || scan.get_i64("registered").map_err(|_| Domain(corrupt()))? < 0
            || scan.get_i64("owner_kind_unresolved").ok() != scan.get_i64("registered").ok()
        {
            return Err(Domain(corrupt()));
        }
        if scan.get_bool("complete").map_err(|_| Domain(corrupt()))? {
            return Ok(());
        }
        let mut after = scan
            .get_str("after")
            .map_err(|_| Domain(corrupt()))?
            .to_owned();
        let mut count = scan.get_i64("registered").map_err(|_| Domain(corrupt()))?;
        for _ in 0..limit {
            let Some(parent) = store
                .sandboxes
                .find_one(doc! {"_id":{"$gt":&after}})
                .sort(doc! {"_id":1})
                .session(&mut *session)
                .await
                .map_err(Driver)?
            else {
                scan.insert("complete", true);
                scan.insert("completed_at", at);
                break;
            };
            let id = parent.get_str("_id").map_err(|_| Domain(corrupt()))?;
            super::super::sandbox_usage::checkpoint::checkpoint(store, session, id, at, false)
                .await?;
            // The checkpoint and this cursor commit together. A failed capture
            // cannot make the scan claim that the source has been registered.
            if self
                .collection(STREAMS)
                .find_one(doc! {"namespace":&self.config.namespace,"sandbox_id":id})
                .session(&mut *session)
                .await
                .map_err(Driver)?
                .is_none()
            {
                return Err(Domain(corrupt()));
            }
            after = id.into();
            count = count.checked_add(1).ok_or_else(|| Domain(corrupt()))?;
        }
        scan.insert("after", after);
        scan.insert("registered", count);
        scan.insert("owner_kind_unresolved", count);
        if saved.is_some() {
            rows.replace_one(doc! {"_id":&self.config.namespace}, scan)
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        } else {
            rows.insert_one(scan)
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        }
        Ok(())
    }
}
