//! Redeem a current owner grant with one majority-committed lifecycle operation.
use super::{policy::*, *};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{commit_transaction, is_confirmed_body_retry, start_transaction, CommitOutcome};
use mongodb::{bson::Timestamp, ClientSession};

#[cfg(test)]
#[path = "writer_proof_tests.rs"]
mod proof_tests;

const TRANSACTION_ATTEMPTS: usize = 8;

pub(crate) struct PreparedCanaryRedemption {
    pub(crate) owner: SubjectId,
    pub(crate) key: String,
    pub(crate) grant_id: String,
    pub(crate) release_key: String,
    pub(crate) policy_revision: i64,
    pub(crate) fresh: Option<FreshCanaryRedemption>,
    pub(crate) input: LifecycleInput,
}

pub(crate) struct FreshCanaryRedemption {
    pub(crate) lane: CanaryLane,
    pub(crate) policy_revision: i64,
    pub(crate) catalog_sha256: String,
    pub(crate) owner_scope_sha256: String,
    pub(crate) release: FrozenRelease,
}

struct Receipt {
    record: LifecycleRecord,
    slot: CanaryGrantSlot,
}

enum Mutation {
    Replay(Box<LifecycleRecord>),
    Fresh(Box<Receipt>),
}

impl MongoAgentLifecycleStore {
    pub(crate) async fn redeem_canary(
        &self,
        database: &Database,
        request: PreparedCanaryRedemption,
    ) -> Result<CreateResult, CanaryWriteError> {
        if !layer2_core::operation::valid_idempotency_key(request.key.as_bytes()) {
            return Err(CanaryWriteError::Refused);
        }
        let request_key = format!("{:x}", Sha256::digest(request.key.as_bytes()));
        let id = uuid::Uuid::new_v4().simple().to_string();
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1 << attempt.min(5))).await;
            }
            let mut session = self.operations.client().start_session().await?;
            start_transaction(&mut session).await?;
            let receipt = match self
                .redeem_once(database, &request, &id, &request_key, &mut session)
                .await
            {
                Ok(Mutation::Replay(record)) => {
                    let _ = session.abort_guarded().await;
                    return Ok(CreateResult::Existing(record));
                }
                Ok(Mutation::Fresh(receipt)) => receipt,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match &error {
                        CanaryWriteError::Driver(error) if is_confirmed_body_retry(error) => {
                            continue
                        }
                        _ => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(CreateResult::Created(id)),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Failed(error) => return Err(error.into()),
                CommitOutcome::Unknown(_) => {
                    let observed = session
                        .operation_time()
                        .ok_or(CanaryWriteError::Unresolved)?;
                    return self
                        .prove_canary_commit(database, &request, &receipt, observed)
                        .await;
                }
            }
        }
        Err(CanaryWriteError::Unresolved)
    }

    async fn redeem_once(
        &self,
        database: &Database,
        request: &PreparedCanaryRedemption,
        operation_id: &str,
        request_key: &str,
        session: &mut ClientSession,
    ) -> Result<Mutation, CanaryWriteError> {
        if crate::store::account_deletions::erasing_in_session(database, &request.owner, session)
            .await?
        {
            return Err(CanaryWriteError::OwnerUnavailable);
        }
        if let Some(record) = self
            .operations
            .find_one(doc! {"owner": request.owner.as_str(), REQUEST_KEY_DIGEST: request_key})
            .session(&mut *session)
            .await?
        {
            exact_replay(&record, request, request_key)?;
            return Ok(Mutation::Replay(Box::new(record)));
        }
        let fresh = request.fresh.as_ref().ok_or(CanaryWriteError::Closed)?;
        if fresh.release.release_key != request.release_key
            || fresh.policy_revision != request.policy_revision
        {
            return Err(CanaryWriteError::Refused);
        }
        fence_owner(database, &request.owner, session).await?;
        fence_policy(
            database,
            fresh.policy_revision,
            &fresh.catalog_sha256,
            &fresh.owner_scope_sha256,
            session,
        )
        .await?;
        crate::store::admission_checks::require_fresh_home(&self.admission_settings, true, session)
            .await
            .map_err(|_| CanaryWriteError::Closed)?;
        let current = slot(database, &request.owner, fresh.lane, session)
            .await?
            .ok_or(CanaryWriteError::Refused)?;
        let digest = image_digest(&fresh.release.image.uri).ok_or_else(invalid_authority)?;
        let next = current.consume(
            &request.grant_id,
            operation_id,
            &digest,
            &fresh.release,
            fresh.policy_revision,
        )?;
        let mut input = request.input.clone();
        input.resolved_image = Some(fresh.release.image.uri.clone());
        input.capacity = Some(fresh.release.capacity);
        input.request.desktop = fresh.lane == CanaryLane::DebianDesktop;
        let internal_key = canary_key::admission_key(operation_id);
        let mut record = LifecycleRecord::for_admission(
            operation_id.to_owned(),
            &request.owner,
            &internal_key,
            input,
        )?;
        record.canary_manifest_digest = Some(digest.clone());
        let mut admission = next.receipt(&request.owner, operation_id, &fresh.release)?;
        admission.fingerprint = admission.fingerprint(&record)?;
        record.canary_redemption_key = Some(CanaryRedemptionKey::new(
            request_key,
            fresh.lane,
            &admission,
        )?);
        record.canary_admission = Some(admission);
        record
            .verify_canary_worker()?
            .ok_or_else(invalid_authority)?;
        if self
            .operations
            .find_one(doc! {"owner": request.owner.as_str(), MANIFEST_DIGEST: digest})
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Err(CanaryWriteError::Refused);
        }
        replace_slot(
            database,
            &request.owner,
            fresh.lane,
            Some(&current),
            &next,
            session,
        )
        .await?;
        self.operations.insert_one(&record).session(session).await?;
        Ok(Mutation::Fresh(Box::new(Receipt { record, slot: next })))
    }

    async fn prove_canary_commit(
        &self,
        database: &Database,
        request: &PreparedCanaryRedemption,
        receipt: &Receipt,
        observed: Timestamp,
    ) -> Result<CreateResult, CanaryWriteError> {
        let proof = tokio::time::timeout(Duration::from_secs(5), async {
            let mut session = self
                .operations
                .client()
                .start_session()
                .snapshot(true)
                .snapshot_time(observed)
                .await?;
            let lane = request
                .fresh
                .as_ref()
                .ok_or(CanaryWriteError::Unresolved)?
                .lane;
            let current = slot(database, &request.owner, lane, &mut session).await?;
            let row = self
                .operations
                .find_one(doc! {"_id": &receipt.record.id, "owner": request.owner.as_str()})
                .read_concern(mongodb::options::ReadConcern::snapshot())
                .session(&mut session)
                .await?;
            let row = row.ok_or(CanaryWriteError::Unresolved)?;
            let expected_key = receipt
                .record
                .canary_redemption_key
                .as_ref()
                .ok_or(CanaryWriteError::Unresolved)?;
            exact_replay(&row, request, &expected_key.sha256)?;
            if current.as_ref() == Some(&receipt.slot)
                && row
                    .canary_admission
                    .as_ref()
                    .map(|value| &value.fingerprint)
                    == receipt
                        .record
                        .canary_admission
                        .as_ref()
                        .map(|value| &value.fingerprint)
            {
                Ok(CreateResult::Created(row.id))
            } else {
                Err(CanaryWriteError::Unresolved)
            }
        })
        .await;
        proof
            .unwrap_or(Err(CanaryWriteError::Unresolved))
            .map_err(|_| CanaryWriteError::Unresolved)
    }
}

fn exact_replay(
    record: &LifecycleRecord,
    request: &PreparedCanaryRedemption,
    request_key: &str,
) -> Result<(), CanaryWriteError> {
    record
        .verify_canary_worker()?
        .ok_or_else(invalid_authority)?;
    let Some(admission) = &record.canary_admission else {
        return Err(CanaryWriteError::Refused);
    };
    let key = record
        .canary_redemption_key
        .as_ref()
        .ok_or_else(invalid_authority)?;
    let mut requested = request.input.request.clone();
    requested.desktop = key.lane == CanaryLane::DebianDesktop;
    if key.sha256 != request_key
        || admission.grant_id != request.grant_id
        || admission.release.release_key != request.release_key
        || admission.policy_revision != request.policy_revision
        || !same_request(&record.input.request, &requested)?
    {
        return Err(CanaryWriteError::Refused);
    }
    Ok(())
}
