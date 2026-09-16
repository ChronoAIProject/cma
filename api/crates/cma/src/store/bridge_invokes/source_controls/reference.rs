use super::*;

impl MongoBridgeInvokeStore {
    /// Refusal-only reader: disabled or locally revoked grants are readable,
    /// but absent/corrupt history never becomes proof of permission denial.
    pub(in crate::store::bridge_invokes) async fn refusal_consent(
        &self,
        grant: &BridgeInvokeGrant,
        original: &SourceControlPolicy,
        receipt: &SourceControlReceipt,
        deadline: tokio::time::Instant,
        session: &mut ClientSession,
    ) -> Result<SourceControlPolicy, MutationError> {
        receipt.require_policy(original)?;
        let collections = Collections::new(self);
        let (current, history) = collections
            .history_for_grant(grant, deadline, session)
            .await?;
        if history.get((original.view.revision - 1) as usize) != Some(original) {
            return Err(BridgeInvokeError::Corrupt.into());
        }
        let retained = collections
            .receipts
            .find_one(doc! {"_id":&receipt.id})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(records::read_receipt)
            .transpose()?
            .ok_or(BridgeInvokeError::Corrupt)?;
        if retained != *receipt {
            return Err(BridgeInvokeError::Corrupt.into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BridgeInvokeError::Unavailable.into());
        }
        current.ok_or(BridgeInvokeError::Corrupt.into())
    }

    /// Read the same complete, bounded policy history as management, while the
    /// caller separately fences the real source grant and runtime session.
    pub(in crate::store::bridge_invokes) async fn reference_consent(
        &self,
        grant: &BridgeInvokeGrant,
        deadline: tokio::time::Instant,
        session: &mut ClientSession,
    ) -> Result<Option<(SourceControlPolicy, SourceControlReceipt)>, MutationError> {
        grant.require_active()?;
        let collections = Collections::new(self);
        let (policy, _) = collections
            .history_for_grant(grant, deadline, session)
            .await?;
        let Some(policy) = policy.filter(|policy| policy.view.enabled) else {
            return Ok(None);
        };
        let receipt = collections
            .receipts
            .find_one(doc! {"_id":&policy.receipt_id})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(records::read_receipt)
            .transpose()?
            .ok_or(BridgeInvokeError::Corrupt)?;
        receipt.require_policy(&policy)?;
        let changed = collections.policies.update_one(
            doc! {"_id":grant.id.as_str(),"record":record_bson(&policy).map_err(|_| BridgeInvokeError::Corrupt)?},
            doc! {"$inc":{"reference_fence":1_i64}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BridgeInvokeError::Unavailable.into());
        }
        Ok(Some((policy, receipt)))
    }
}
