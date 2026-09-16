use super::*;

impl MongoBridgeReplyToolStore {
    pub(super) async fn apply_recovery(
        &self,
        write: OutboxRecoveryWrite,
    ) -> Result<OutboxRecoveryReceipt, OutboxManagementError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            write.require_current()?;
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
            let result = self.recovery_transaction(&write, &mut session).await;
            let receipt = match result {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            if let Err(error) = write.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    self.wake.notify_one();
                    return Ok(receipt);
                }
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable.into());
                }
            }
        }
        Err(OutboxError::Unavailable.into())
    }
    async fn recovery_transaction(
        &self,
        write: &OutboxRecoveryWrite,
        session: &mut ClientSession,
    ) -> Result<OutboxRecoveryReceipt, ManagementMutationError> {
        let manager = write.management();
        self.fence_manager(manager, session).await?;
        if let Some(receipt) = self
            .recovery_in(manager, write.receipt_id(), write.request_sha256(), session)
            .await?
        {
            return Ok(receipt);
        }
        let document = self.items.find_one(doc! {"_id":&write.request().item_id,"record.source.pin.bridge_id":manager.record().id.as_str()})
            .session(&mut *session).await?.ok_or(OutboxManagementError::NotFound)?;
        let mut item = records::read_item(&document)?;
        let previous_revision = item.revision;
        let now = now_ms();
        write.apply(&mut item, now)?;
        if let Some(validation) = write.validation() {
            self.fence_delivery(manager.record(), &item.source, validation, session)
                .await?;
        }
        if item.revision != previous_revision {
            self.replace_item(previous_revision, &item, session).await?;
        }
        let receipt = OutboxRecoveryReceipt {
            id: write.receipt_id().into(),
            bridge_id: manager.record().id.clone(),
            request_sha256: write.request_sha256().into(),
            actor: manager.actor().clone(),
            action: write.request().action,
            result: (&item).into(),
            created_at_ms: now,
        };
        self.recoveries
            .insert_one(
                doc! {"_id":&receipt.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
                "bridge_id":receipt.bridge_id.as_str(),"request_sha256":&receipt.request_sha256,
                "record":record_bson(&receipt).map_err(|_| OutboxError::Corrupt)?},
            )
            .session(session)
            .await?;
        write.require_current()?;
        Ok(receipt)
    }
}
