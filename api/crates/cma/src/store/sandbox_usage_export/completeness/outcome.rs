use super::*;
use mongodb::bson::DateTime;

fn prefix(namespace: &str, stream: &str) -> String {
    format!("{}/{stream}/", hash(namespace.as_bytes()))
}

fn outcome_key(namespace: &str, stream: &str, minute: u64, basis: Basis) -> Document {
    doc! {"usage_completeness_outcome_v1":format!("{}{minute:016x}/{basis:?}",prefix(namespace,stream))}
}

fn retry_key(namespace: &str, stream: &str, minute: u64) -> Document {
    doc! {"usage_completeness_retry_v1":format!("{}{minute:016x}",prefix(namespace,stream))}
}

impl CaptureStore {
    pub(in crate::store::sandbox_usage_export) async fn record_outcome(
        &self,
        session: &mut ClientSession,
        source: &str,
        minute: u64,
        basis: Basis,
        outcome: MinuteOutcome,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let id = outcome_key(&self.config.namespace, source, minute, basis);
        let rows = self.collection(INVENTORY);
        let previous = rows
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        let attempts = previous
            .as_ref()
            .and_then(|row| row.get_i64("attempts").ok())
            .unwrap_or_default()
            .checked_add(1)
            .ok_or_else(|| Domain(corrupt()))?;
        let (status, reason) = match outcome {
            MinuteOutcome::Staged => ("staged", None),
            MinuteOutcome::Retry(reason) => ("pending", Some(reason)),
            MinuteOutcome::Held(reason) => ("held", Some(reason)),
        };
        rows.replace_one(doc! {"_id":&id},doc! {"_id":id,"document_schema_version":1_i64,"status":status,"reason":reason,"attempts":attempts,"minute":number(minute).map_err(Domain)?,"updated_at":DateTime::now()})
            .upsert(previous.is_none()).session(&mut *session).await.map_err(Driver)?;
        if basis == Basis::Allocated {
            let id = retry_key(&self.config.namespace, source, minute);
            if matches!(outcome, MinuteOutcome::Retry(_)) {
                let delay = (60_i64 << attempts.clamp(0, 6)).min(3600);
                rows.update_one(doc! {"_id":&id},doc! {"$set":{"document_schema_version":1_i64,"minute":number(minute).map_err(Domain)?,"due_at":DateTime::from_millis(DateTime::now().timestamp_millis()+delay*1000)}})
                    .upsert(true).session(&mut *session).await.map_err(Driver)?;
            } else {
                rows.delete_one(doc! {"_id":id})
                    .session(&mut *session)
                    .await
                    .map_err(Driver)?;
            }
        }
        Ok(())
    }

    pub(super) async fn due_allocated_retry(
        &self,
        session: &mut ClientSession,
        source: &str,
    ) -> Result<Option<u64>, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let prefix = prefix(&self.config.namespace, source);
        let upper = format!("{}0", prefix.trim_end_matches('/'));
        let row=self.collection(INVENTORY).find_one(doc! {"_id":{"$gte":{"usage_completeness_retry_v1":prefix},"$lt":{"usage_completeness_retry_v1":upper}},"due_at":{"$lte":DateTime::now()}})
            .sort(doc! {"_id":1}).session(&mut *session).await.map_err(Driver)?;
        row.map(|row| {
            row.get_i64("minute")
                .map_err(|_| Domain(corrupt()))
                .and_then(|minute| u64::try_from(minute).map_err(|_| Domain(corrupt())))
        })
        .transpose()
    }
}
