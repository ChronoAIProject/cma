use super::*;

pub(super) enum ReadRequest<'a> {
    Items { after: Option<&'a str>, limit: u32 },
    Item(&'a str),
    Receipt { id: &'a str, digest: &'a str },
}
#[derive(Default)]
pub(super) struct ReadResult {
    pub items: Vec<OutboxItem>,
    pub receipt: Option<OutboxRecoveryReceipt>,
}

impl MongoBridgeReplyToolStore {
    pub(super) async fn managed_read(
        &self,
        manager: &BridgeManagementLease,
        request: ReadRequest<'_>,
    ) -> Result<ReadResult, OutboxManagementError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            manager.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = self.read_transaction(manager, &request, &mut session).await;
            let value = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            if let Err(error) = manager.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error.into());
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    manager.require_current()?;
                    return Ok(value);
                }
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable.into());
                }
            }
        }
        Err(OutboxError::Unavailable.into())
    }
    async fn read_transaction(
        &self,
        manager: &BridgeManagementLease,
        request: &ReadRequest<'_>,
        session: &mut ClientSession,
    ) -> Result<ReadResult, ManagementMutationError> {
        self.fence_manager(manager, session).await?;
        let mut result = ReadResult::default();
        match request {
            ReadRequest::Receipt { id, digest } => {
                result.receipt = self.recovery_in(manager, id, digest, session).await?
            }
            ReadRequest::Item(id) => {
                if let Some(document) = self
                    .items
                    .find_one(
                        doc! {"_id":*id,"record.source.pin.bridge_id":manager.record().id.as_str()},
                    )
                    .session(&mut *session)
                    .await?
                {
                    let item = records::read_item(&document)?;
                    require_source(manager, &item)?;
                    result.items.push(item);
                }
            }
            ReadRequest::Items { after, limit } => {
                let mut filter = doc! {"record.source.pin.bridge_id":manager.record().id.as_str()};
                if let Some(after) = after {
                    filter.insert("_id", doc! {"$gt":*after});
                }
                let mut cursor = self
                    .items
                    .find(filter)
                    .sort(doc! {"_id":1})
                    .limit(i64::from(*limit) + 1)
                    .session(&mut *session)
                    .await?;
                while cursor.advance(&mut *session).await? {
                    let document = cursor.deserialize_current()?;
                    let item = records::read_item(&document)?;
                    require_source(manager, &item)?;
                    result.items.push(item);
                }
            }
        }
        manager.require_current()?;
        Ok(result)
    }
}

#[async_trait::async_trait]
impl BridgeOutboxManagementStore for MongoBridgeReplyToolStore {
    async fn list(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<OutboxItem>, OutboxManagementError> {
        if !(1..=100).contains(&limit) {
            return Err(OutboxManagementError::Invalid);
        }
        Ok(self
            .managed_read(manager, ReadRequest::Items { after, limit })
            .await?
            .items)
    }
    async fn item(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<OutboxItem>, OutboxManagementError> {
        Ok(self
            .managed_read(manager, ReadRequest::Item(id))
            .await?
            .items
            .into_iter()
            .next())
    }
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        request_sha256: &str,
    ) -> Result<Option<OutboxRecoveryReceipt>, OutboxManagementError> {
        Ok(self
            .managed_read(
                manager,
                ReadRequest::Receipt {
                    id,
                    digest: request_sha256,
                },
            )
            .await?
            .receipt)
    }
    async fn apply(
        &self,
        write: OutboxRecoveryWrite,
    ) -> Result<OutboxRecoveryReceipt, OutboxManagementError> {
        self.apply_recovery(write).await
    }
}
