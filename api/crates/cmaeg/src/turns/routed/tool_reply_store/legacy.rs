//! A confirmed OFF-writer attempt is archived before a keyed claim can displace it.
use super::*;
const KIND: &str = "ordinary_route_legacy_tool_confirmation_v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    kind: String,
    root: String,
    target: String,
    owner: String,
    request_hash: String,
    retained_until: i64,
    operation: String,
    receipt: StoredReceipt,
}
impl Confirmation {
    fn id(record: &Record, operation: &str) -> String {
        storage::identifier(&(KIND, &record.target, operation))
    }
    fn valid(&self, record: &Record, operation: &str) -> bool {
        self.kind == KIND
            && self.root == record.root
            && self.target == record.target
            && self.owner == record.owner
            && self.request_hash == record.request_hash
            && self.retained_until == record.retained_until
            && self.operation == operation
            && matches!(self.receipt.delivery, DeliveryOutcome::Confirmed { .. })
    }
}
impl ReplyStore {
    pub(super) async fn legacy_confirmation(
        &self,
        record: &Record,
        operation: &str,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        if record.legacy_confirmation.as_deref() != Some(operation) {
            return Ok(None);
        }
        let (saved, version) = self
            .0
            .read::<Confirmation>(&Confirmation::id(record, operation))
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        if version != 1 || !saved.valid(record, operation) {
            return Err(ExecutionError::Corrupt);
        }
        self.0
            .receipts(operation, &record.owner, record.retained_until)
            .load(&saved.receipt)
            .await
            .map(Some)
            .map_err(|_| ExecutionError::Unavailable)
    }
    pub(super) async fn preserve_legacy(&self, record: &mut Record) -> Result<(), ExecutionError> {
        let Some(attempt) = record.tool.as_ref().filter(|a| a.content_hash.is_none()) else {
            return Ok(());
        };
        let Some(receipt) = attempt
            .receipt
            .as_ref()
            .filter(|r| matches!(r.delivery, DeliveryOutcome::Confirmed { .. }))
        else {
            return Ok(());
        };
        if record
            .legacy_confirmation
            .as_ref()
            .is_some_and(|id| id != &attempt.operation)
        {
            return Err(ExecutionError::Corrupt);
        }
        // A second full receipt in the guard could exceed its 40 KiB bound.
        // Store one immutable archive first; its small pointer is then written
        // atomically with the new claim. A failed/uncertain archive cannot erase
        // the original slot or grant a new send, and an orphan simply expires.
        self.0
            .receipts(&attempt.operation, &record.owner, record.retained_until)
            .load(receipt)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let id = Confirmation::id(record, &attempt.operation);
        let expected = Confirmation {
            kind: KIND.into(),
            root: record.root.clone(),
            target: record.target.clone(),
            owner: record.owner.clone(),
            request_hash: record.request_hash.clone(),
            retained_until: record.retained_until,
            operation: attempt.operation.clone(),
            receipt: receipt.clone(),
        };
        if let Some((saved, version)) = self.0.read::<Confirmation>(&id).await? {
            if version != 1 || storage::bytes(&saved)? != storage::bytes(&expected)? {
                return Err(ExecutionError::Corrupt);
            }
        } else if let Err(error) = self
            .0
            .write_fenced(
                &self.1,
                &id,
                &record.owner,
                record.retained_until,
                &expected,
                None,
            )
            .await
        {
            match self.0.read::<Confirmation>(&id).await {
                Ok(Some((saved, 1))) if storage::bytes(&saved)? == storage::bytes(&expected)? => {}
                _ => return Err(error),
            }
        }
        record.legacy_confirmation = Some(attempt.operation.clone());
        Ok(())
    }
}
