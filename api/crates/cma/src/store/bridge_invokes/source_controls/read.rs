use super::*;
type Snapshot = (Option<SourceControlPolicy>, Option<SourceControlReceipt>);

impl MongoBridgeInvokeStore {
    pub(super) async fn read_source_controls(
        &self,
        access: &SourceControlAccess,
        lookup: Option<(&str, &str)>,
    ) -> Result<Snapshot, BridgeInvokeError> {
        for _ in 0..8 {
            access.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            let result = self
                .source_control_read_transaction(access, lookup, &mut session)
                .await;
            let _ = session.abort_transaction().await;
            match result {
                Ok(value) => {
                    access.require_current()?;
                    return Ok(value);
                }
                Err(MutationError::Mongo(error)) if is_transient(&error) => continue,
                Err(error) => return Err(error.into_invoke()),
            }
        }
        Err(BridgeInvokeError::Unavailable)
    }
    async fn source_control_read_transaction(
        &self,
        access: &SourceControlAccess,
        lookup: Option<(&str, &str)>,
        session: &mut ClientSession,
    ) -> Result<Snapshot, MutationError> {
        self.source_control_access(access, session).await?;
        let collections = Collections::new(self);
        let (policy, history) = collections.history(access, session).await?;
        let receipt = match lookup {
            Some((id, digest)) => {
                collections
                    .recover(access, &history, id, digest, session)
                    .await?
            }
            None => None,
        };
        Ok((policy, receipt))
    }
}
impl Collections {
    pub(super) async fn history(
        &self,
        access: &SourceControlAccess,
        session: &mut ClientSession,
    ) -> Result<(Option<SourceControlPolicy>, Vec<SourceControlPolicy>), MutationError> {
        access.require_current()?;
        let result = self
            .history_for_grant(access.grant(), access.management().deadline(), session)
            .await?;
        access.require_current()?;
        Ok(result)
    }
    pub(super) async fn history_for_grant(
        &self,
        grant: &BridgeInvokeGrant,
        deadline: tokio::time::Instant,
        session: &mut ClientSession,
    ) -> Result<(Option<SourceControlPolicy>, Vec<SourceControlPolicy>), MutationError> {
        grant.validate()?;
        let current = self
            .policies
            .find_one(doc! {"_id":grant.id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(records::read_policy)
            .transpose()?;
        let mut cursor = self
            .audits
            .find(doc! {"grant_id":grant.id.as_str()})
            .sort(doc! {"revision":1_i32})
            .limit((MAX_SOURCE_CONTROL_REVISIONS + 1) as i64)
            .session(&mut *session)
            .await?;
        let mut history: Vec<SourceControlPolicy> = Vec::new();
        while cursor.advance(&mut *session).await? {
            if tokio::time::Instant::now() >= deadline {
                return Err(BridgeInvokeError::Unavailable.into());
            }
            let policy = records::read_audit(&cursor.deserialize_current()?)?;
            policy.view.require_grant(grant)?;
            if policy.view.revision != history.len() as u64 + 1 {
                return Err(BridgeInvokeError::Corrupt.into());
            }
            let receipt = self
                .receipts
                .find_one(doc! {"_id":&policy.receipt_id})
                .session(&mut *session)
                .await?
                .as_ref()
                .map(records::read_receipt)
                .transpose()?
                .ok_or(BridgeInvokeError::Corrupt)?;
            receipt.require_policy(&policy)?;
            if !policy.view.enabled && history.last().is_none_or(|p| !p.view.enabled) {
                return Err(BridgeInvokeError::Corrupt.into());
            }
            history.push(policy);
        }
        let receipts = self
            .receipts
            .count_documents(doc! {"grant_id":grant.id.as_str()})
            .limit(MAX_SOURCE_CONTROL_REVISIONS + 1)
            .session(&mut *session)
            .await?;
        if receipts != history.len() as u64
            || history.last() != current.as_ref()
            || history.len() > MAX_SOURCE_CONTROL_REVISIONS as usize
        {
            return Err(BridgeInvokeError::Corrupt.into());
        }
        if let Some(current) = &current {
            current.view.require_grant(grant)?;
        }
        Ok((current, history))
    }
    pub(super) async fn recover(
        &self,
        access: &SourceControlAccess,
        history: &[SourceControlPolicy],
        id: &str,
        digest: &str,
        session: &mut ClientSession,
    ) -> Result<Option<SourceControlReceipt>, MutationError> {
        let Some(document) = self
            .receipts
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
        else {
            return Ok(None);
        };
        let receipt = records::read_receipt(&document)?;
        access.require_receipt(&receipt)?;
        let policy = history
            .get((receipt.result.revision - 1) as usize)
            .ok_or(BridgeInvokeError::Corrupt)?;
        receipt.require_policy(policy)?;
        if source_control_request_digest(&receipt.request)? != digest {
            return Err(BridgeInvokeError::IdempotencyConflict.into());
        }
        Ok(Some(receipt))
    }
}
