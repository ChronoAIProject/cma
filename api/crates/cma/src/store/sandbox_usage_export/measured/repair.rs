use super::freeze::{Manifest, MeasuredPublication};
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeasuredRepair {
    pub source_stream: String,
    pub minute: u64,
    pub evidence_digest: String,
    pub predecessor_event: String,
}

impl MeasuredRepair {
    fn key(&self, namespace: &str) -> Result<String, SandboxStoreError> {
        if !uuid(&self.source_stream)
            || !uuid(&self.predecessor_event)
            || !digest(&self.evidence_digest)
            || !self.minute.is_multiple_of(60)
            || self.minute > i64::MAX as u64 / 1000 - 60
        {
            return Err(corrupt());
        }
        Ok(format!(
            "{}{}",
            keys::prefix("repair", namespace),
            hash(&serde_json::to_vec(self).map_err(|_| corrupt())?)
        ))
    }
}

impl CaptureStore {
    pub(crate) async fn repair_measured(
        &self,
        request: &MeasuredRepair,
        apply: bool,
    ) -> Result<bool, SandboxStoreError> {
        let key = request.key(&self.config.namespace)?;
        if self
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&key),"complete":true})
            .await
            .map_err(unavailable)?
            .is_some()
        {
            return Ok(true);
        }
        let stream_key = keys::stream(&self.config.namespace, &request.source_stream);
        if !apply {
            let row = self
                .collection(INVENTORY)
                .find_one(doc! {"_id":keys::id(&stream_key)})
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?;
            self.repair_publication(&MeasuredStream::read(&row)?, request)
                .await?;
            return Ok(false);
        }
        self.collection(INVENTORY)
            .update_one(
                doc! {"_id":keys::id(&key)},
                doc! {"$setOnInsert":{
                "document_schema_version":1_i64,"record":encode(request)?,"complete":false}},
            )
            .upsert(true)
            .await
            .map_err(unavailable)?;
        let lease = self
            .claim_measured(&stream_key)
            .await?
            .ok_or(SandboxStoreError::Conflict)?;
        let result = async {
            let publication = self.repair_publication(&lease.stream,request).await?;
            transaction::run(self,(self,&lease,&publication,&key), |&(store,lease,publication,key),session| Box::pin(async move {
                use TransactionEventError::{Domain,Driver};
                let row = store.collection(INVENTORY).find_one(doc! {"_id":keys::id(key)})
                    .session(&mut *session).await.map_err(Driver)?.ok_or_else(|| Domain(corrupt()))?;
                if row.get_bool("complete").map_err(|_| Domain(corrupt()))? { return Ok(true); }
                store.fence_measured(session,lease).await?;
                store.publish_fenced(session,lease,publication,"algorithm_correction",false).await?;
                let matched = store.collection(INVENTORY).update_one(doc! {"_id":keys::id(key),"complete":false},
                    doc! {"$set":{"complete":true,"result_manifest":&publication.digest}})
                    .session(session).await.map_err(Driver)?.matched_count;
                if matched != 1 { return Err(Domain(SandboxStoreError::Conflict)); }
                Ok(true)
            })).await
        }.await;
        let release = self.release_measured(&lease).await;
        let complete = result?;
        release?;
        Ok(complete)
    }

    async fn repair_publication(
        &self,
        stream: &MeasuredStream,
        request: &MeasuredRepair,
    ) -> Result<MeasuredPublication, SandboxStoreError> {
        let row = self
            .collection(EVIDENCE)
            .find_one(doc! {"_id":format!("measured:{}:{}",stream.id,request.evidence_digest)})
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let bytes = row.get_binary_generic("manifest").map_err(unavailable)?;
        if hash(bytes) != request.evidence_digest {
            return Err(corrupt());
        }
        let retained: Manifest = serde_json::from_slice(bytes).map_err(|_| corrupt())?;
        let original = &retained.input;
        if retained.version != 1
            || retained.hold.is_some()
            || original.minute != request.minute
            || original.stream.id != stream.id
            || original.stream.source_stream != request.source_stream
            || original.stream.source_epoch != stream.source_epoch
            || original.stream.owner != stream.owner
            || original.stream.owner_issuer != stream.owner_issuer
            || original.stream.namespace != stream.namespace
            || original.stream.sandbox_id != stream.sandbox_id
        {
            return Err(corrupt());
        }
        let frozen = retained.frozen.as_ref().ok_or_else(corrupt)?;
        let recomputed = frozen.recompute_retained().map_err(|_| corrupt())?;
        let head: FactHead = decode(
            &self
                .collection(INVENTORY)
                .find_one(doc! {"_id":keys::id(&keys::head(&stream.id,request.minute))})
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?,
        )?;
        if head.event_id != request.predecessor_event || head.resource != original.resource {
            return Err(SandboxStoreError::Conflict);
        }
        let mut manifest = retained;
        manifest.frozen = Some(recomputed);
        manifest.retained_from = Some(request.evidence_digest.clone());
        manifest.input.stream = stream.clone();
        manifest.input.predecessor = Some(head.clone());
        let result_digest = hash(
            &serde_json::to_vec(&(
                manifest.frozen.as_ref().ok_or_else(corrupt)?.fact(),
                &manifest.input.resource,
            ))
            .map_err(|_| corrupt())?,
        );
        let bytes = serde_json::to_vec(&manifest).map_err(|_| corrupt())?;
        Ok(MeasuredPublication {
            manifest,
            digest: hash(&bytes),
            bytes,
            result_digest: Some(result_digest),
            fact_id: head.fact_id,
            event_id: uuid::Uuid::new_v4().to_string(),
            opening_id: uuid::Uuid::new_v4().to_string(),
        })
    }
}
