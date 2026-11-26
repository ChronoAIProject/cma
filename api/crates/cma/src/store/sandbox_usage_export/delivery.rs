use super::*;
use crate::sandbox_usage_export::Receipt;

mod cursor_v1;
use cursor_v1::StoredCursorV1;
#[cfg(test)]
pub(super) mod compatibility_tests;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{Bson, DateTime},
    options::ReturnDocument,
};
use serde::{Deserialize, Serialize};

const LEASE_MS: i64 = 120_000;

#[derive(Clone, Debug)]
pub(crate) struct DeliveryAttempt {
    pub event: FrozenUsageEvent,
    pub destination: String,
    pub attempt_id: String,
    pub generation: i64,
    pub audit: bool,
    pub retained_receipt: Option<Receipt>,
}

pub(crate) enum DeliveryOutcome {
    Accepted(Receipt),
    Retry { seconds: u64, reason: &'static str },
    Hold(&'static str),
    ProducerForbidden,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    destination: String,
    attempt_id: String,
    generation: i64,
    lease_until: DateTime,
    due_at: DateTime,
    status: Status,
    reason: Option<String>,
    receipt: Option<Receipt>,
    cursor: Option<StoredCursorV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Pending,
    Accepted,
    Held,
    ProducerForbidden,
}

impl MongoSandboxStore {
    pub(crate) fn usage_capture_store(&self) -> Option<CaptureStore> {
        self.usage_capture.clone()
    }
}

impl CaptureStore {
    pub(crate) async fn export_streams(
        &self,
        after: &str,
    ) -> Result<Vec<String>, SandboxStoreError> {
        let mut cursor = self
            .collection(STREAMS)
            .find(doc! {"namespace":&self.config.namespace,"_id":{"$gt":after}})
            .sort(doc! {"_id":1})
            .limit(32)
            .await
            .map_err(unavailable)?;
        let mut streams = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            let stream = records::Stream::read(&row)?;
            if stream.namespace != self.config.namespace {
                return Err(corrupt());
            }
            streams.push(stream.id);
        }
        streams.extend(self.measured_delivery_page(after).await?);
        streams.sort();
        streams.dedup();
        streams.truncate(32);
        Ok(streams)
    }

    pub(crate) async fn producer_blocked(
        &self,
        destination: &str,
    ) -> Result<bool, SandboxStoreError> {
        self.bind_producer(destination).await
    }

    pub(crate) async fn claim_delivery(
        &self,
        stream: &str,
        destination: &str,
    ) -> Result<Option<DeliveryAttempt>, SandboxStoreError> {
        if self.bind_producer(destination).await? {
            return Ok(None);
        }
        let pending = self
            .collection(OUTBOX)
            .find_one(doc! {
                "stream_id":stream,"delivery.status":{"$ne":"accepted"}
            })
            .sort(doc! {"sequence":1})
            .await
            .map_err(unavailable)?;
        let audit = pending.is_none();
        let row = match pending {
            Some(row) => row,
            None => match self
                .collection(OUTBOX)
                .find_one(doc! {"stream_id":stream})
                .sort(doc! {"sequence":-1})
                .await
                .map_err(unavailable)?
            {
                Some(row) => row,
                None => return Ok(None),
            },
        };
        let event = FrozenUsageEvent::read(&row)?;
        if event.namespace != self.config.namespace {
            return Err(corrupt());
        }
        if event.sequence > 1 {
            let predecessor = self
                .collection(OUTBOX)
                .find_one(doc! {"stream_id":stream,"sequence":records::number(event.sequence - 1)?})
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?;
            if state(&predecessor)?.is_none_or(|state| state.status != Status::Accepted) {
                return Err(corrupt());
            }
        }
        let previous = state(&row)?;
        if let Some(previous) = &previous {
            if previous.destination != destination
                || previous.status
                    != if audit {
                        Status::Accepted
                    } else {
                        Status::Pending
                    }
            {
                return Ok(None);
            }
        }
        let generation = previous
            .as_ref()
            .map_or(Some(1), |v| v.generation.checked_add(1))
            .ok_or_else(corrupt)?;
        let attempt_id = uuid::Uuid::new_v4().to_string();
        let next = State {
            version: 1,
            destination: destination.into(),
            attempt_id: attempt_id.clone(),
            generation,
            lease_until: DateTime::from_millis(0),
            due_at: DateTime::from_millis(0),
            status: Status::Pending,
            reason: None,
            receipt: previous.as_ref().and_then(|v| v.receipt.clone()),
            cursor: previous.and_then(|v| v.cursor),
        };
        let old = row
            .get("delivery")
            .cloned()
            .unwrap_or_else(|| Bson::Document(doc! {"$exists":false}));
        let changed = self.collection(OUTBOX).find_one_and_update(doc! {
            "_id":&event.event_id,"digest":&event.digest,"payload":row.get("payload").ok_or_else(corrupt)?,
            "delivery":old,
            "$expr":{"$and":[
                {"$lte":[{"$ifNull":["$delivery.lease_until",DateTime::from_millis(0)]},"$$NOW"]},
                {"$lte":[{"$ifNull":["$delivery.due_at",DateTime::from_millis(0)]},"$$NOW"]}
            ]}
        }, vec![doc! {"$set":{"delivery":{"$literal":records::encode(&next)?}}},doc! {"$set":{
            "delivery.lease_until":{"$dateAdd":{"startDate":"$$NOW","unit":"millisecond","amount":LEASE_MS}},
            "delivery.due_at":"$$NOW"
        }}]).return_document(ReturnDocument::After).await.map_err(unavailable)?;
        let Some(changed) = changed else {
            return Ok(None);
        };
        let written = state(&changed)?.ok_or_else(corrupt)?;
        if written.generation != generation || written.attempt_id != attempt_id {
            return Err(corrupt());
        }
        Ok(Some(DeliveryAttempt {
            event,
            destination: destination.into(),
            attempt_id,
            generation,
            audit: audit || next.receipt.is_some(),
            retained_receipt: next.receipt,
        }))
    }

    pub(crate) async fn finish_delivery(
        &self,
        attempt: &DeliveryAttempt,
        outcome: DeliveryOutcome,
    ) -> Result<bool, SandboxStoreError> {
        let (status, reason, receipt, seconds) = match outcome {
            DeliveryOutcome::Accepted(receipt) => {
                receipt.validate(&attempt.event).map_err(|_| corrupt())?;
                if attempt
                    .retained_receipt
                    .as_ref()
                    .is_some_and(|old| old != &receipt)
                {
                    return Err(corrupt());
                }
                (Status::Accepted, None, Some(receipt), 600)
            }
            DeliveryOutcome::Retry { seconds, reason } => {
                (Status::Pending, Some(reason), None, seconds.clamp(1, 3600))
            }
            DeliveryOutcome::Hold(reason) => (Status::Held, Some(reason), None, 0),
            DeliveryOutcome::ProducerForbidden => {
                self.forbid_producer(&attempt.destination).await?;
                (
                    Status::ProducerForbidden,
                    Some("producer_forbidden"),
                    None,
                    0,
                )
            }
        };
        let updated = self.collection(OUTBOX).update_one(doc! {
            "_id":&attempt.event.event_id,"digest":&attempt.event.digest,
            "delivery.version":1_i32,"delivery.destination":&attempt.destination,
            "delivery.attempt_id":&attempt.attempt_id,"delivery.generation":attempt.generation,
            "delivery.status":"pending","$expr":{"$gt":["$delivery.lease_until","$$NOW"]}
        },vec![doc! {"$set":{
            "delivery.status":{"$literal":mongodb::bson::serialize_to_bson(&status).map_err(|_| corrupt())?},
            "delivery.reason":{"$literal":mongodb::bson::serialize_to_bson(&reason).map_err(|_| corrupt())?},
            "delivery.receipt": if receipt.is_some() {
                Bson::Document(doc! {"$literal":mongodb::bson::serialize_to_bson(&receipt).map_err(|_| corrupt())?})
            } else { Bson::String("$delivery.receipt".into()) },
            "delivery.lease_until":"$$NOW",
            "delivery.due_at":{"$dateAdd":{"startDate":"$$NOW","unit":"second","amount":seconds as i64}}
        }}]).await.map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }

    pub(crate) async fn event_digest(
        &self,
        stream: &str,
        sequence: u64,
    ) -> Result<Option<String>, SandboxStoreError> {
        let row = self
            .collection(OUTBOX)
            .find_one(doc! {"stream_id":stream,"sequence":records::number(sequence)?})
            .await
            .map_err(unavailable)?;
        row.map(|row| FrozenUsageEvent::read(&row).map(|event| event.digest))
            .transpose()
    }
}

fn state(row: &Document) -> Result<Option<State>, SandboxStoreError> {
    let Some(value) = row.get("delivery") else {
        return Ok(None);
    };
    let state: State =
        mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| corrupt())?;
    if state.version != 1
        || state.generation < 1
        || (state.status == Status::Accepted && state.receipt.is_none())
        || !uuid::Uuid::parse_str(&state.attempt_id).is_ok_and(|v| v.get_version_num() == 4)
        || state.destination.len() != 64
    {
        return Err(corrupt());
    }
    if let Some(receipt) = &state.receipt {
        receipt
            .validate(&FrozenUsageEvent::read(row)?)
            .map_err(|_| corrupt())?;
    }
    Ok(Some(state))
}
