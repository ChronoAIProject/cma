//! A correction may concern a minute outside the ordinary replay horizon.
use super::{freeze::Manifest, *};
use crate::{
    sandbox_usage_export::completeness_contract::Basis,
    store::sandbox_usage_export::completeness::MinuteOutcome,
};
use mongodb::{bson::DateTime, ClientSession};

fn rebuild_key(namespace: &str, source: &str, minute: u64) -> Document {
    doc! {"usage_completeness_rebuild_v1":format!("{}/{source}/{minute:016x}",hash(namespace.as_bytes()))}
}

impl CaptureStore {
    pub(super) async fn record_measured_completeness(
        &self,
        session: &mut ClientSession,
        stream: &MeasuredStream,
        minute: u64,
        head: &FactHead,
        outcome: MinuteOutcome,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        self.record_outcome(
            session,
            &stream.source_stream,
            minute,
            Basis::Measured,
            outcome,
        )
        .await?;
        let id = rebuild_key(&stream.namespace, &stream.source_stream, minute);
        let collection = self.collection(INVENTORY);
        if matches!(outcome, MinuteOutcome::Retry(_)) {
            collection.update_one(doc! {"_id":&id},doc! {"$set":{"document_schema_version":1_i64,"source":&stream.source_stream,"minute":number(minute).map_err(Domain)?,"head":encode(head).map_err(Domain)?,"due_at":DateTime::from_millis(DateTime::now().timestamp_millis()+60_000)}})
                .upsert(true).session(&mut *session).await.map_err(Driver)?;
        } else {
            collection
                .delete_one(doc! {"_id":id})
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        }
        Ok(())
    }

    pub(crate) async fn rebuild_measured_completeness(&self) -> Result<(), SandboxStoreError> {
        if !self.completeness_enabled() {
            return Ok(());
        }
        let prefix = format!("{}/", hash(self.config.namespace.as_bytes()));
        let upper = format!("{}0", prefix.trim_end_matches('/'));
        let Some(row)=self.collection(INVENTORY).find_one(doc! {"_id":{"$gte":{"usage_completeness_rebuild_v1":prefix},"$lt":{"usage_completeness_rebuild_v1":upper}},"due_at":{"$lte":DateTime::now()}})
            .sort(doc! {"_id":1}).await.map_err(unavailable)? else {return Ok(());};
        let source = row.get_str("source").map_err(unavailable)?.to_owned();
        let minute =
            u64::try_from(row.get_i64("minute").map_err(unavailable)?).map_err(unavailable)?;
        if !uuid(&source) || !minute.is_multiple_of(60) {
            return Err(corrupt());
        }
        let id = rebuild_key(&self.config.namespace, &source, minute);
        if row.get_document("_id").ok() != Some(&id) {
            return Err(corrupt());
        }
        let Some(lease) = self
            .claim_measured(&keys::stream(&self.config.namespace, &source))
            .await?
        else {
            self.collection(INVENTORY).update_one(doc! {"_id":&id,"due_at":{"$lte":DateTime::now()}},doc! {"$set":{"due_at":DateTime::from_millis(DateTime::now().timestamp_millis()+30_000)}}).await.map_err(unavailable)?;
            return Ok(());
        };
        let result = transaction::run(
            self,
            (self, &lease, minute),
            |&(store, lease, minute), session| {
                Box::pin(async move { store.rebuild_measured_in(session, lease, minute).await })
            },
        )
        .await;
        let release = self.release_measured(&lease).await;
        result.and(release)
    }

    async fn rebuild_measured_in(
        &self,
        session: &mut ClientSession,
        lease: &MeasuredLease,
        minute: u64,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        self.fence_measured(session, lease).await?;
        let stream = &lease.stream;
        let id = rebuild_key(&stream.namespace, &stream.source_stream, minute);
        let Some(rebuild) = self
            .collection(INVENTORY)
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await
            .map_err(Driver)?
        else {
            return Ok(());
        };
        let queued: FactHead = mongodb::bson::deserialize_from_bson(
            rebuild
                .get("head")
                .cloned()
                .ok_or_else(|| Domain(corrupt()))?,
        )
        .map_err(|_| Domain(corrupt()))?;
        let head: FactHead = self
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&keys::head(&stream.id,minute))})
            .session(&mut *session)
            .await
            .map_err(Driver)?
            .as_ref()
            .map(decode)
            .transpose()
            .map_err(Domain)?
            .ok_or_else(|| Domain(corrupt()))?;
        if head != queued && (head.fact_id != queued.fact_id || head.revision <= queued.revision) {
            return Err(Domain(corrupt()));
        }
        // A retained fact survives an unsuccessful refresh. Its original
        // manifest cannot clear a newer authoritative minute hold.
        if self
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&keys::hold(&stream.id, minute))})
            .session(&mut *session)
            .await
            .map_err(Driver)?
            .is_some()
        {
            return self
                .record_measured_completeness(
                    session,
                    stream,
                    minute,
                    &head,
                    MinuteOutcome::Retry("refresh_hold_pending"),
                )
                .await;
        }
        let row = self
            .collection(EVIDENCE)
            .find_one(doc! {"_id":format!("measured:{}:{}",stream.id,head.evidence_id)})
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        let Some(row) = row else {
            return self
                .record_measured_completeness(
                    session,
                    stream,
                    minute,
                    &head,
                    MinuteOutcome::Retry("retained_manifest_pending"),
                )
                .await;
        };
        let bytes = row
            .get_binary_generic("manifest")
            .map_err(|_| Domain(corrupt()))?;
        if hash(bytes) != head.evidence_id {
            return Err(Domain(corrupt()));
        }
        let manifest: Manifest = serde_json::from_slice(bytes).map_err(|_| Domain(corrupt()))?;
        let input = &manifest.input;
        if manifest.version != 1
            || manifest.hold.is_some()
            || input.minute != minute
            || input.stream.id != stream.id
            || input.stream.source_stream != stream.source_stream
            || input.stream.source_epoch != stream.source_epoch
            || input.stream.owner != stream.owner
            || input.stream.namespace != stream.namespace
            || input.resource != head.resource
        {
            return Err(Domain(corrupt()));
        }
        let frozen = manifest.frozen.as_ref().ok_or_else(|| Domain(corrupt()))?;
        let input_digest = hash(
            &serde_json::to_vec(&(frozen.fact(), &input.resource))
                .map_err(|_| Domain(corrupt()))?,
        );
        if input_digest != head.input_digest {
            return Err(Domain(corrupt()));
        }
        let outcome = self
            .stage_measured_completeness(session, stream, &manifest, &head)
            .await?;
        self.record_measured_completeness(session, stream, minute, &head, outcome)
            .await
    }
}
