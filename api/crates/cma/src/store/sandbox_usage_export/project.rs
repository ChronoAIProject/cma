use super::super::sandbox_events::TransactionEventError;
use super::super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use super::{
    payload,
    records::{decode, encode, hash, number, Captured, Stream},
    *,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::{
    bson::{spec::BinarySubtype, Binary},
    ClientSession,
};

impl CaptureStore {
    pub(crate) async fn project_one(&self, id: &str, now: u64) -> Result<bool, SandboxStoreError> {
        let ids = payload::Ids::new();
        for attempt in 0..TRANSACTION_ATTEMPTS {
            retry_delay(attempt).await;
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let projected = match self.project_in(&mut session, id, now, &ids).await {
                Ok(projected) => projected,
                Err(error) => {
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
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(projected),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error))
                }
            }
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn project_in(
        &self,
        session: &mut ClientSession,
        id: &str,
        now: u64,
        ids: &payload::Ids,
    ) -> Result<bool, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let Some(document) = self
            .collection(CAPTURES)
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await
            .map_err(Driver)?
        else {
            return Ok(false);
        };
        let capture = read_capture(&document).map_err(Domain)?;
        if document
            .get_bool("complete")
            .map_err(|_| Domain(corrupt()))?
        {
            return Ok(false);
        }
        let mut stream = Stream::read(
            &self
                .collection(STREAMS)
                .find_one(doc! {"_id":&capture.stream_id})
                .session(&mut *session)
                .await
                .map_err(Driver)?
                .ok_or_else(|| Domain(corrupt()))?,
        )
        .map_err(Domain)?;
        if stream.owner != capture.source.owner.as_str()
            || stream.sandbox_id != capture.source.sandbox_id.as_str()
            || capture.source.revision < stream.first_revision
            || capture.source.revision > stream.last_revision
        {
            return Err(Domain(corrupt()));
        }
        let missing_binding = capture.source.ended_at > capture.source.started_at
            && ((capture.source.allocation.running
                && (capture.resource.runtime_id.is_none()
                    || capture.resource.runtime_generation.is_none()))
                || (capture.source.allocation.disk_gib > 0
                    && (capture.resource.home_id.is_none()
                        || capture.resource.home_generation.is_none())));
        if capture.source_gap || missing_binding {
            self.collection(CAPTURES).update_one(doc! {"_id":id,"complete":false},
                doc! {"$set":{"projection_hold":if capture.source_gap {"source_gap"} else {"binding_missing"}}})
                .session(&mut *session).await.map_err(Driver)?;
            return Ok(false);
        }
        if stream.projected_revision != capture.previous_revision {
            return Ok(false);
        }
        let cursor = u64::try_from(
            document
                .get_i64("next_second")
                .map_err(|_| Domain(corrupt()))?,
        )
        .map_err(|_| Domain(corrupt()))?;
        let batch = layer2_core::sandbox_usage_evidence::project(&capture.source, cursor)
            .map_err(|_| Domain(corrupt()))?;
        if stream.sequence == 0 {
            let event = payload::opening(&stream, &capture, &ids.opening, now).map_err(Domain)?;
            self.append(session, &mut stream, &capture, -1, &event)
                .await?;
        }
        let part = (cursor / 60 - capture.source.started_at / 60)
            / layer2_core::sandbox_usage_evidence::MAX_FACTS as u64;
        let event =
            payload::usage(&stream, &capture, &batch.facts, ids, part, now).map_err(Domain)?;
        let facts = batch.facts.iter().zip(&ids.facts).map(|(fact, fact_id)| {
            Ok(doc! {
                "_id":fact_id, "document_schema_version":1_i64,"stream_id":&stream.id,
                "owner":&stream.owner,"sandbox_id":&stream.sandbox_id,
                "source_id":id,"part":number(fact.part).map_err(Domain)?,
                "record":encode(fact).map_err(Domain)?,"source_digest":document.get_str("source_digest").map_err(|_| Domain(corrupt()))?,
            })
        }).collect::<Result<Vec<_>, TransactionEventError>>()?;
        if !facts.is_empty() {
            self.collection(EVIDENCE)
                .insert_many(facts)
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        }
        self.append(
            session,
            &mut stream,
            &capture,
            number(part).map_err(Domain)?,
            &event,
        )
        .await?;
        if batch.complete {
            stream.projected_revision = Some(capture.source.revision);
        }
        let changed = self.collection(CAPTURES).update_one(doc! {"_id":id,"next_second":number(cursor).map_err(Domain)?,"complete":false},
            doc! {"$set":{"next_second":number(batch.next_second).map_err(Domain)?,"complete":batch.complete}})
            .session(&mut *session).await.map_err(Driver)?;
        if changed.matched_count != 1 {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        let changed = self
            .collection(STREAMS)
            .replace_one(doc! {"_id":&stream.id}, stream.document().map_err(Domain)?)
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        if changed.matched_count != 1 {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        Ok(true)
    }

    async fn append(
        &self,
        session: &mut ClientSession,
        stream: &mut Stream,
        capture: &Captured,
        part: i64,
        event: &serde_json::Value,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let bytes = serde_json_canonicalizer::to_vec(event).map_err(|_| Domain(corrupt()))?;
        if bytes.len() > 512 * 1024 {
            return Err(Domain(corrupt()));
        }
        let digest = hash(&bytes);
        stream.sequence = stream
            .sequence
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| Domain(corrupt()))?;
        self.collection(OUTBOX)
            .insert_one(doc! {
                "_id":event["event_id"].as_str().ok_or_else(|| Domain(corrupt()))?,
                "document_schema_version":1_i64,"stream_id":&stream.id,
                "owner":&stream.owner,"sandbox_id":&stream.sandbox_id,
                "sequence":number(stream.sequence).map_err(Domain)?,
                "source_id":capture.source.transaction_id(),"part":part,
                "digest":&digest,"payload":Binary { subtype: BinarySubtype::Generic, bytes },
                "charge_disposition":"nonbillable_shadow",
            })
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        stream.previous_digest = Some(digest);
        Ok(())
    }
}

pub(super) fn read_capture(document: &Document) -> Result<Captured, SandboxStoreError> {
    let capture: Captured = decode(document)?;
    let event = &capture.source;
    if document.get_str("_id").ok() != Some(event.transaction_id().as_str())
        || document.get_str("sandbox_id").ok() != Some(event.sandbox_id.as_str())
        || document.get_str("owner").ok() != Some(event.owner.as_str())
        || document.get_str("stream_id").ok() != Some(capture.stream_id.as_str())
        || document.get_i64("revision").ok() != Some(number(event.revision)?)
        || document.get_str("source_digest").ok()
            != Some(hash(&serde_json::to_vec(&capture).map_err(|_| corrupt())?).as_str())
    {
        return Err(corrupt());
    }
    Ok(capture)
}
