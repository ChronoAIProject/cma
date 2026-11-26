use super::*;
use crate::store::agent_ownership::FenceError;
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    agent_capacity_recovery::source::envelope_filter,
    sandbox_mongo::{
        commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
    },
};

pub(super) struct Observation {
    store: MongoSandboxStore,
    owner: OwnershipToken,
    claimed: Agent,
    key: Key,
    filter: Document,
    parent_filter: Document,
    prior: Option<Bson>,
    id: String,
    observed_at: DateTime,
    until: DateTime,
    lease_until: DateTime,
    #[cfg(test)]
    gate: Option<Arc<TestGate>>,
}

pub(super) async fn capture(
    store: &MongoSandboxStore,
    claimed: &Agent,
    owner: &OwnershipToken,
) -> Result<Option<Observation>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let Some((row, now)) = server_row(store, &claimed.id, &mut session).await? else {
        return Ok(None);
    };
    if read_agent(&row)? != *claimed {
        return Ok(None);
    }
    let epoch = i64::try_from(owner.epoch).map_err(unavailable)?;
    if row.get_i64("agent_manager_work_epoch").ok() != Some(epoch) {
        return Ok(None);
    }
    let Some(lease) = &claimed.work.lease else {
        return Ok(None);
    };
    let Some(lease_ms) = layer2_core::seconds_from_rfc3339(&lease.expires_at)
        .and_then(|s| s.checked_mul(1000))
        .and_then(|ms| i64::try_from(ms).ok())
    else {
        return Ok(None);
    };
    let lease_until = DateTime::from_millis(lease_ms);
    if lease_until <= now {
        return Ok(None);
    }
    let Some(parent_row) = store
        .sandboxes
        .find_one(doc! {"_id": claimed.sandbox_id.as_str()})
        .session(&mut session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(None);
    };
    let parent = read_sandbox(&parent_row)?;
    let Some(key) = Key::capture(claimed, &row, &parent) else {
        return Ok(None);
    };
    let Some(until) = now
        .timestamp_millis()
        .checked_add(HOLD_MS)
        .map(DateTime::from_millis)
    else {
        return Ok(None);
    };
    let mut filter = envelope_filter(claimed, &row)?;
    for field in [
        "agent_manager_work_epoch",
        "agent_command_admission_fence",
        "work_lease_holder",
        "work_lease_fence",
        "work_lease_expires_at",
        FIELD,
    ] {
        exact(&mut filter, &row, field);
    }
    let runtime = parent.runtime.as_ref().unwrap();
    let revision = i64::try_from(parent.revision).map_err(unavailable)?;
    let mut parent_filter = doc! {
        "_id": parent.id.as_str(), "owner": parent.owner.as_str(),
        "document_schema_version": parent_row.get("document_schema_version").cloned().unwrap_or(Bson::Null),
        "revision": revision, "record.revision": revision,
        "record.runtime.runtime_id": &runtime.runtime_id, "record.runtime.boot_id": runtime.boot_id.clone(),
        "record.runtime.runtime_version": &runtime.runtime_version,
        "record.runtime.provision_generation": key.provision_generation,
        "record.runtime.bound_at": &runtime.bound_at,
    };
    let lifecycle = parent_row
        .get_document("record")
        .and_then(|record| record.get_document("lifecycle"))
        .map_err(unavailable)?;
    parent_filter.insert(
        "record.lifecycle.runtime_generation_high_water",
        lifecycle
            .get("runtime_generation_high_water")
            .cloned()
            .unwrap_or_else(|| Bson::Document(doc! {"$exists": false})),
    );
    Ok(Some(Observation {
        store: store.clone(),
        owner: owner.clone(),
        claimed: claimed.clone(),
        key,
        filter,
        parent_filter,
        prior: row.get(FIELD).cloned(),
        id: uuid::Uuid::new_v4().to_string(),
        observed_at: now,
        until,
        lease_until,
        #[cfg(test)]
        gate: None,
    }))
}

fn exact(filter: &mut Document, row: &Document, field: &str) {
    filter.insert(
        field,
        row.get(field)
            .cloned()
            .unwrap_or_else(|| Bson::Document(doc! {"$exists": false})),
    );
}

impl Observation {
    fn hint(&self, evidence: &FirstOwnershipFailure) -> Option<Hint> {
        let owner = evidence.owner();
        let receiver = evidence.receiver()?;
        if owner.agent_id != self.owner.agent_id.as_str()
            || owner.instance_id != self.owner.instance_id
            || owner.epoch != self.owner.epoch
            || owner.protocol != self.owner.protocol
            || receiver.runtime_id() != self.key.runtime_id
            || evidence.confirmation_sent()
            || evidence.previously_confirmed()
        {
            return None;
        }
        // Native transport proves the Pod/container route for this handle.
        // The supervisor boot UUID is a separate identity, retained in the
        // captured key and parent CAS rather than compared to a Pod UID.
        let failure = evidence.failure();
        if failure.phase != OwnershipPhase::Challenge {
            return None;
        }
        let reason = match failure.reason {
            OwnershipFailureReason::MissingRoute if failure.http_status == Some(404) => {
                Reason::CapabilityMissing
            }
            OwnershipFailureReason::ProtocolMismatch
                if evidence.challenge_valid()
                    && failure
                        .protocol
                        .is_some_and(|protocol| protocol != OWNER_PROTOCOL_VERSION) =>
            {
                Reason::ChallengeProtocolMismatch
            }
            OwnershipFailureReason::LeaseMismatch
                if evidence.challenge_valid()
                    && failure.protocol == Some(OWNER_PROTOCOL_VERSION)
                    && failure.lease_ms.is_some_and(|ms| ms != RECEIVER_LEASE_MS) =>
            {
                Reason::ChallengeLifetimeMismatch
            }
            _ => return None,
        };
        Some(Hint {
            version: 1,
            observation: self.id.clone(),
            reason,
            key: self.key.clone(),
            owner_protocol: i64::from(OWNER_PROTOCOL_VERSION),
            receiver_lease_ms: RECEIVER_LEASE_MS as i64,
            retry_classifier_version: 1,
            observed_at: self.observed_at,
            until: self.until,
            owner_epoch: self.owner.epoch.try_into().ok()?,
            claimed_revision: self.claimed.revision.try_into().ok()?,
        })
    }

    async fn write(&self, hint: &Document) -> Result<HintWriteResult, Error> {
        let store = &self.store;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            crate::store::contention::backoff(attempt).await;
            let mut session = store.client.start_session().await?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    continue;
                }
                return Err(error.into());
            }
            match self.apply(hint, &mut session).await {
                Ok(HintWriteResult::Recorded) => (),
                result => {
                    if session.abort_guarded().await.is_err() {
                        emit("unavailable", None);
                    }
                    match result {
                        Err(Error::Driver(error)) if is_transient(&error) => continue,
                        other => return other,
                    }
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(HintWriteResult::Recorded),
                CommitOutcome::Retry(_) => continue,
                // Never replay an observation after an uncertain commit outcome.
                CommitOutcome::Unknown(_) => return Ok(HintWriteResult::Unavailable),
                CommitOutcome::Failed(error) => return Err(error.into()),
            }
        }
        Ok(HintWriteResult::Unavailable)
    }

    async fn apply(
        &self,
        hint: &Document,
        session: &mut ClientSession,
    ) -> Result<HintWriteResult, Error> {
        let store = &self.store;
        #[cfg(test)]
        self.pause("before_fence").await;
        // Keep the driver error typed so this bounded write loop retries a
        // positively known abort; real owner refusals stay terminal.
        match store
            .agent_ownership
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?
            .fence_transaction_typed(&self.owner, session)
            .await
        {
            Ok(()) => {}
            Err(FenceError::Driver(error)) => return Err(Error::Driver(error)),
            Err(FenceError::Domain(error)) => return Err(Error::Domain(error)),
        }
        let Some(parent_row) = store
            .sandboxes
            .find_one(self.parent_filter.clone())
            .session(&mut *session)
            .await?
        else {
            return Ok(HintWriteResult::Stale);
        };
        let parent = read_sandbox(&parent_row)?;
        #[cfg(test)]
        self.pause("parent_read").await;
        let marker = match parent_row.get("runtime_writer_claim_fence") {
            None => 0,
            Some(Bson::Int64(value)) if *value >= 0 && *value < i64::MAX => *value,
            _ => return Ok(HintWriteResult::Stale),
        };
        let mut filter = self.parent_filter.clone();
        exact(&mut filter, &parent_row, "runtime_writer_claim_fence");
        let changed = store
            .sandboxes
            .update_one(
                filter,
                doc! {"$set": {"runtime_writer_claim_fence": marker + 1}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Ok(HintWriteResult::Stale);
        }
        let Some((row, now)) = server_row(store, &self.claimed.id, session).await? else {
            return Ok(HintWriteResult::Stale);
        };
        let agent = read_agent(&row)?;
        if now < self.observed_at || now >= self.until || now >= self.lease_until {
            return Ok(HintWriteResult::Stale);
        }
        if holds(&row, &agent, &parent, now) {
            return Ok(HintWriteResult::AlreadyPresent);
        }
        if agent != self.claimed
            || row.get(FIELD) != self.prior.as_ref()
            || Key::capture(&agent, &row, &parent).as_ref() != Some(&self.key)
        {
            return Ok(HintWriteResult::Stale);
        }
        #[cfg(test)]
        self.pause("agent_read").await;
        let mut filter = self.filter.clone();
        filter.insert(
            "$expr",
            doc! {"$and": [
                {"$lte": [self.observed_at, policy_now(store)]}, {"$gt": [self.until, policy_now(store)]},
                {"$gt": [self.lease_until, "$$NOW"]},
            ]},
        );
        let changed = store
            .agents
            .update_one(filter, doc! {"$set": {FIELD: hint.clone()}})
            .session(session)
            .await?;
        #[cfg(test)]
        self.pause("agent_written").await;
        Ok(if changed.matched_count == 1 {
            HintWriteResult::Recorded
        } else {
            HintWriteResult::Stale
        })
    }
}

#[cfg(test)]
pub(in crate::store) struct TestGate {
    point: &'static str,
    armed: std::sync::atomic::AtomicBool,
    pub(in crate::store) arrived: tokio::sync::Notify,
    pub(in crate::store) resume: tokio::sync::Notify,
}

#[cfg(test)]
impl Observation {
    async fn pause(&self, point: &str) {
        if let Some(gate) = &self.gate {
            if gate.point == point && gate.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                gate.arrived.notify_one();
                gate.resume.notified().await;
            }
        }
    }
}

#[derive(Debug)]
enum Error {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

/// Storage tests qualify the transaction independently of transport classification.
/// Composed worker tests use the real evidence port instead of this fixture seam.
#[cfg(test)]
pub(in crate::store) struct TestObservation(Observation);

#[cfg(test)]
impl TestObservation {
    pub(in crate::store) fn policy_time(&mut self, now: DateTime) {
        self.0.store.ownership_retry_hint_now = Some(now);
    }
    pub(in crate::store) fn pause_at(&mut self, point: &'static str) -> Arc<TestGate> {
        let gate = Arc::new(TestGate {
            point,
            armed: true.into(),
            arrived: Default::default(),
            resume: Default::default(),
        });
        self.0.gate = Some(gate.clone());
        gate
    }
    pub(in crate::store) async fn capture(
        store: &MongoSandboxStore,
        agent: &Agent,
        owner: &OwnershipToken,
    ) -> Self {
        Self(
            capture(store, agent, owner)
                .await
                .unwrap()
                .expect("eligible captured claim"),
        )
    }
    pub(in crate::store) fn document(&self) -> Document {
        mongodb::bson::serialize_to_document(&Hint {
            version: 1,
            observation: self.0.id.clone(),
            reason: Reason::CapabilityMissing,
            key: self.0.key.clone(),
            owner_protocol: 1,
            receiver_lease_ms: RECEIVER_LEASE_MS as i64,
            retry_classifier_version: 1,
            observed_at: self.0.observed_at,
            until: self.0.until,
            owner_epoch: self.0.owner.epoch as i64,
            claimed_revision: self.0.claimed.revision as i64,
        })
        .unwrap()
    }
    pub(in crate::store) async fn write(&self) -> HintWriteResult {
        match tokio::time::timeout(BUDGET, self.0.write(&self.document())).await {
            Ok(Ok(result)) => result,
            Ok(Err(Error::Domain(SandboxStoreError::Conflict))) => HintWriteResult::Stale,
            result => panic!("hint transaction failed: {result:?}"),
        }
    }
}
impl From<SandboxStoreError> for Error {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

#[async_trait::async_trait]
impl CapturedOwnershipProbe for Observation {
    async fn record(&self, failure: &FirstOwnershipFailure) -> HintWriteResult {
        if !self.store.ownership_retry_hints_active() {
            return HintWriteResult::NotApplicable;
        }
        let Some(hint) = self.hint(failure) else {
            return HintWriteResult::NotApplicable;
        };
        let Ok(document) = mongodb::bson::serialize_to_document(&hint) else {
            return HintWriteResult::NotApplicable;
        };
        if mongodb::bson::serialize_to_vec(&document).map_or(true, |bytes| bytes.len() > MAX_BYTES)
        {
            emit("oversized", Some(hint.reason));
            return HintWriteResult::NotApplicable;
        }
        let result = match tokio::time::timeout(BUDGET, self.write(&document)).await {
            Ok(Ok(result)) => result,
            Ok(Err(Error::Domain(SandboxStoreError::Conflict))) => HintWriteResult::Stale,
            _ => HintWriteResult::Unavailable,
        };
        emit(
            match result {
                HintWriteResult::Recorded => "recorded",
                HintWriteResult::AlreadyPresent => "already_present",
                HintWriteResult::NotApplicable => "mismatched",
                HintWriteResult::Stale => "stale_write_rejected",
                HintWriteResult::Unavailable => "unavailable",
            },
            Some(hint.reason),
        );
        result
    }
}
