use super::freeze::MeasuredPublication;
use super::*;
use crate::store::{sandbox_metric_bindings, usage_reports};
use serde_json::json;

impl CaptureStore {
    pub(crate) async fn publish_measured(
        &self,
        lease: &MeasuredLease,
        publication: &MeasuredPublication,
    ) -> Result<bool, SandboxStoreError> {
        transaction::run(
            self,
            (self, lease, publication),
            |&(store, lease, publication), session| {
                Box::pin(store.publish_measured_in(session, lease, publication))
            },
        )
        .await
    }

    pub(super) async fn publish_measured_in(
        &self,
        session: &mut ClientSession,
        lease: &MeasuredLease,
        publication: &MeasuredPublication,
    ) -> Result<bool, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let input = &publication.manifest.input;
        if input.stream != lease.stream {
            return Err(Domain(corrupt()));
        }
        self.fence_measured(session, lease).await?;
        let matched = self
            .collection(usage_reports::CONTROL)
            .update_one(
                doc! {"_id":"observer","revision":input.mapping_revision},
                doc! {"$inc":{"control_write_fence":1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(Driver)?
            .matched_count;
        if matched != 1 {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        for binding in &input.bindings {
            let matched=self.collection(sandbox_metric_bindings::COLLECTION).update_one(
                doc! {"_id":&binding.id,"revision":number(binding.revision).map_err(Domain)?,"record":encode(binding).map_err(Domain)?},
                doc! {"$inc":{"measured_write_fence":1_i64}}).session(&mut *session).await.map_err(Driver)?.matched_count;
            if matched != 1 {
                return Err(Domain(SandboxStoreError::Conflict));
            }
        }
        self.publish_fenced(session, lease, publication, "late_sample", true)
            .await
    }

    pub(super) async fn publish_fenced(
        &self,
        session: &mut ClientSession,
        lease: &MeasuredLease,
        publication: &MeasuredPublication,
        reason: &str,
        advance: bool,
    ) -> Result<bool, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let input = &publication.manifest.input;
        let mut stream = lease.stream.clone();
        let key = keys::head(&stream.id, input.minute);
        let actual: Option<FactHead> = self
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&key)})
            .session(&mut *session)
            .await
            .map_err(Driver)?
            .as_ref()
            .map(decode)
            .transpose()
            .map_err(Domain)?;
        if actual != input.predecessor {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        let unchanged = actual
            .as_ref()
            .is_some_and(|h| Some(&h.input_digest) == publication.result_digest.as_ref());
        let held = publication.manifest.hold.is_some();
        if !unchanged || held || !advance {
            self.retain_manifest(session, publication).await?;
        }
        if held {
            let key = format!("hold/{}/{:016x}", stream.id, input.minute);
            self.collection(INVENTORY).replace_one(doc! {"_id":keys::id(&key)}, doc! {
                "_id":keys::id(&key),"document_schema_version":1_i64,"stream_id":&stream.id,
                "minute":number(input.minute).map_err(Domain)?,"evidence_id":&publication.digest,"reason":&publication.manifest.hold,
            }).upsert(true).session(&mut *session).await.map_err(Driver)?;
        } else if !unchanged {
            if stream.sequence == 0 {
                let event = payload::open(
                    payload::event(
                        &stream,
                        input,
                        &publication.opening_id,
                        &publication.digest,
                        lease.now,
                    )
                    .map_err(Domain)?,
                    &stream,
                );
                self.append_measured(session, &mut stream, &publication.digest, -1, &event)
                    .await?;
            }
            let revision = actual
                .as_ref()
                .map_or(Some(1), |h| h.revision.checked_add(1))
                .filter(|r| *r <= i64::MAX as u64)
                .ok_or_else(|| Domain(corrupt()))?;
            let fact = payload::fact(
                publication
                    .manifest
                    .frozen
                    .as_ref()
                    .ok_or_else(|| Domain(corrupt()))?
                    .fact(),
                &input.resource,
                &publication.fact_id,
                revision,
            );
            let mut event = payload::event(
                &stream,
                input,
                &publication.event_id,
                &publication.digest,
                lease.now,
            )
            .map_err(Domain)?;
            if let Some(head) = &actual {
                event["kind"] = "correction".into();
                event["correction"] = json!({"fact_id":head.fact_id,"expected_predecessor_event_id":head.event_id,
                    "expected_predecessor_digest":head.digest,"expected_evidence_revision":head.revision.to_string(),"replacement":fact,"reason":reason});
            } else {
                event["kind"] = "usage".into();
                event["facts"] = json!([fact]);
            }
            self.append_measured(session, &mut stream, &publication.digest, 0, &event)
                .await?;
            let head = FactHead {
                fact_id: publication.fact_id.clone(),
                revision,
                event_id: publication.event_id.clone(),
                digest: stream
                    .previous_digest
                    .clone()
                    .ok_or_else(|| Domain(corrupt()))?,
                evidence_id: publication.digest.clone(),
                input_digest: publication
                    .result_digest
                    .clone()
                    .ok_or_else(|| Domain(corrupt()))?,
                resource: input.resource.clone(),
            };
            self.collection(INVENTORY)
                .replace_one(
                    doc! {"_id":keys::id(&key)},
                    document(&key, &head).map_err(Domain)?,
                )
                .upsert(actual.is_none())
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        }
        if !held {
            self.collection(INVENTORY)
                .delete_one(
                    doc! {"_id":keys::id(&format!("hold/{}/{:016x}",stream.id,input.minute))},
                )
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        }
        if advance {
            if input.replay {
                let next = input.minute + 60;
                stream.replay_minute = if next >= stream.next_minute {
                    stream
                        .capture_start
                        .max(lease.now.saturating_sub(30 * 86400) / 60 * 60)
                        .min(stream.next_minute)
                } else {
                    next
                };
                stream.replay_turn = false;
            } else {
                stream.next_minute = input.minute + 60;
                stream.replay_turn = stream.replay_minute < stream.next_minute;
            }
        }
        let changed = self
            .collection(INVENTORY)
            .update_one(
                lease.filter(),
                doc! {"$set":{"record":encode(&stream).map_err(Domain)?}},
            )
            .session(session)
            .await
            .map_err(Driver)?;
        if changed.matched_count != 1 {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        Ok(!held && !unchanged)
    }

    async fn retain_manifest(
        &self,
        session: &mut ClientSession,
        publication: &MeasuredPublication,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let stream = &publication.manifest.input.stream;
        let id = format!("measured:{}:{}", stream.id, publication.digest);
        let document = doc! {"_id":&id,"document_schema_version":1_i64,"stream_id":&stream.id,"owner":&stream.owner,
        "sandbox_id":&stream.sandbox_id,"source_id":&publication.digest,"part":0_i64,
        "source_digest":&publication.digest,"manifest":bytes(publication.bytes.clone())};
        if let Some(old) = self
            .collection(EVIDENCE)
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await
            .map_err(Driver)?
        {
            if old != document {
                return Err(Domain(corrupt()));
            }
        } else {
            self.collection(EVIDENCE)
                .insert_one(document)
                .session(session)
                .await
                .map_err(Driver)?;
        }
        Ok(())
    }

    async fn append_measured(
        &self,
        session: &mut ClientSession,
        stream: &mut MeasuredStream,
        source_id: &str,
        part: i64,
        event: &serde_json::Value,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let payload = serde_json_canonicalizer::to_vec(event).map_err(|_| Domain(corrupt()))?;
        if payload.len() > 512 * 1024 {
            return Err(Domain(corrupt()));
        }
        let digest = hash(&payload);
        stream.sequence += 1;
        self.collection(OUTBOX).insert_one(doc! {"_id":event["event_id"].as_str().ok_or_else(|| Domain(corrupt()))?,
            "document_schema_version":1_i64,"stream_id":&stream.id,"owner":&stream.owner,"sandbox_id":&stream.sandbox_id,
            "sequence":number(stream.sequence).map_err(Domain)?,"source_id":source_id,"part":part,"digest":&digest,"payload":bytes(payload),
            "charge_disposition":"nonbillable_shadow"}).session(session).await.map_err(Driver)?;
        stream.previous_digest = Some(digest);
        Ok(())
    }
}
