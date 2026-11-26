use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::{
    resource_access::ResourceAccessStoreError,
    sandbox::store::{ProvenanceBackfillOutcome, ProvenanceBackfillProgress},
    AgentId,
};
use mongodb::{bson::doc, options::ReturnDocument, ClientSession};

use super::{
    model::{Control, Counts, Phase, CONTROL},
    *,
};
use crate::store::{
    agent_provenance_backfill::apply,
    resource_access::ReadError,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
        MongoSandboxStore, TRANSACTION_ATTEMPTS,
    },
};

const LEASE_MS: i64 = 30_000;

impl MongoSandboxStore {
    pub(in crate::store) async fn provenance_progress_step(
        &self,
        owner: &str,
    ) -> Result<Option<ProvenanceBackfillProgress>> {
        if uuid::Uuid::parse_str(owner).is_err() {
            return Err(corrupt());
        }
        let database = self.client.database(&self.agents.namespace().db);
        if !exists(&database).await? {
            return Ok(None);
        }
        // Read/validate before claiming. Existing collection without its control
        // or completion receipt is corruption, never permission for legacy mode.
        let before = read(&database).await?;
        if let Some(id) = &before.last_receipt {
            receipt(&database, id).await?;
        }
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| unavailable())?;
            start_transaction(&mut session)
                .await
                .map_err(|_| unavailable())?;
            let result = execute(self, &database, owner, &mut session).await;
            let progress = match result {
                Ok(progress) => progress,
                Err(Failure::Observation {
                    control,
                    id,
                    reason,
                }) => {
                    let _ = session.abort_guarded().await;
                    return record_exception(self, &database, owner, &control, &id, reason)
                        .await
                        .map(Some);
                }
                Err(Failure::Driver(error)) if is_transient(&error) || is_duplicate_key(&error) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(match error {
                        Failure::Store(error) => error,
                        _ => unavailable(),
                    });
                }
            };
            if progress == ProvenanceBackfillProgress::Busy {
                let _ = session.abort_guarded().await;
                return Ok(Some(progress));
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(Some(progress)),
                CommitOutcome::Retry(_) => continue,
                // Never replay an unknown insertion as a retained observation.
                // Effect, cursor and counts committed together. The next call
                // starts from durable progress, or retries the uncommitted item.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => return Err(unavailable()),
            }
        }
        Err(unavailable())
    }
}

enum Failure {
    Driver(mongodb::error::Error),
    Store(SandboxStoreError),
    Observation {
        control: Box<Control>,
        id: AgentId,
        reason: &'static str,
    },
}
impl From<mongodb::error::Error> for Failure {
    fn from(e: mongodb::error::Error) -> Self {
        Self::Driver(e)
    }
}
impl From<SandboxStoreError> for Failure {
    fn from(e: SandboxStoreError) -> Self {
        Self::Store(e)
    }
}

async fn claim(
    database: &Database,
    owner: &str,
    session: &mut ClientSession,
) -> std::result::Result<Option<(Control, Control)>, Failure> {
    let collection = rows(database);
    let before = Control::decode(
        collection
            .find_one(doc! {"_id":CONTROL})
            .session(&mut *session)
            .await?
            .ok_or_else(corrupt)?,
    )?;
    if before.revision == i64::MAX || before.fence == i64::MAX {
        return Err(corrupt().into());
    }
    let mut filter = before.expected();
    filter.insert(
        "$expr",
        doc! {"$and":[
            {"$lte":["$next_sweep_at","$$NOW"]},
            {"$or":[{"$eq":["$owner",{"$literal":owner}]},{"$lte":["$lease_until","$$NOW"]}]}
        ]},
    );
    let claimed = collection.find_one_and_update(filter, vec![doc! {"$set":{
        "owner":{"$literal":owner}, "lease_until":{"$add":["$$NOW",LEASE_MS]},
        "updated_at":"$$NOW", "revision":{"$add":["$revision",1_i64]},
        "fence":{"$cond":[{"$eq":["$owner",{"$literal":owner}]},"$fence",{"$add":["$fence",1_i64]}]}
    }}]).return_document(ReturnDocument::After).session(session).await?;
    let Some(row) = claimed else {
        return Ok(None);
    };
    let mut control = Control::decode(row)?;
    if control.phase != Phase::Running {
        control.run = uuid::Uuid::new_v4().to_string();
        control.phase = Phase::Running;
        control.cursor = None;
        control.counts = Counts::default();
        control.started_at = Some(control.updated_at);
    }
    Ok(Some((before, control)))
}

async fn execute(
    store: &MongoSandboxStore,
    database: &Database,
    owner: &str,
    session: &mut ClientSession,
) -> std::result::Result<ProvenanceBackfillProgress, Failure> {
    let Some((before, mut control)) = claim(database, owner, session).await? else {
        return Ok(ProvenanceBackfillProgress::Busy);
    };
    let next = store
        .agents
        .find_one(doc! {"_id":{"$type":"string", "$gt":control.cursor.as_deref().unwrap_or("")}})
        .projection(doc! {"_id":1})
        .sort(doc! {"_id":1})
        .session(&mut *session)
        .await?;
    let Some(next) = next else {
        control.phase = Phase::SweepFinished;
        let receipt_id = format!("sweep:{}", control.run);
        control.last_receipt = Some(receipt_id.clone());
        control.next_sweep_at =
            mongodb::bson::DateTime::from_millis(control.updated_at.timestamp_millis() + 300_000);
        // Permanent sweep evidence must not retain the private resume cursor
        // or lease-holder identity. Keep both on the live control row.
        let mut receipt = control.clone();
        receipt.id = receipt_id;
        receipt.cursor = None;
        receipt.owner = None;
        rows(database)
            .insert_one(receipt.document()?)
            .session(&mut *session)
            .await?;
        persist(database, &before, &control, owner, session).await?;
        return Ok(ProvenanceBackfillProgress::SweepFinished);
    };
    let id = AgentId(next.get_str("_id").map_err(|_| corrupt())?.into());
    let outcome = match apply(store, &id, session).await {
        Ok(ProvenanceBackfillOutcome::Recorded) => "recorded",
        Ok(ProvenanceBackfillOutcome::Retained) => "retained",
        Err(ReadError::Domain(ResourceAccessStoreError::NotFound)) => {
            return Err(Failure::Observation {
                control: Box::new(before),
                id,
                reason: "missing",
            })
        }
        Err(ReadError::Domain(
            ResourceAccessStoreError::Corrupt | ResourceAccessStoreError::Capacity,
        )) => {
            return Err(Failure::Observation {
                control: Box::new(before),
                id,
                reason: "corrupt",
            })
        }
        Err(ReadError::Driver(error)) => return Err(Failure::Driver(error)),
        Err(ReadError::Domain(_)) => return Err(unavailable().into()),
    };
    control.counts.observe(outcome)?;
    control.cursor = Some(id.to_string());
    persist(database, &before, &control, owner, session).await?;
    Ok(ProvenanceBackfillProgress::Advanced)
}

async fn persist(
    database: &Database,
    before: &Control,
    control: &Control,
    owner: &str,
    session: &mut ClientSession,
) -> std::result::Result<(), Failure> {
    // Claim already holds a write lock until this transaction commits/aborts.
    // Require the same revision, owner, fence and a live server-time lease.
    let filter = doc! {"_id":CONTROL,"revision":before.revision + 1,
    "owner":owner,"fence":control.fence,"$expr":{"$gt":["$lease_until","$$NOW"]}};
    if rows(database)
        .replace_one(filter, control.document()?)
        .session(session)
        .await?
        .matched_count
        != 1
    {
        return Err(unavailable().into());
    }
    Ok(())
}

async fn record_exception(
    store: &MongoSandboxStore,
    database: &Database,
    owner: &str,
    expected: &Control,
    id: &AgentId,
    reason: &str,
) -> Result<ProvenanceBackfillProgress> {
    // The rejected effect transaction has been aborted. Preserve an observation
    // only if no other worker advanced that exact cursor/revision in the meantime.
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| unavailable())?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable())?;
    let result: std::result::Result<(), Failure> = async {
        let Some((before, mut control)) = claim(database, owner, &mut session).await? else {
            return Err(unavailable().into());
        };
        if before.revision != expected.revision
            || before.run != expected.run
            || before.cursor != expected.cursor
        {
            return Err(unavailable().into());
        }
        control.counts.observe(reason)?;
        control.cursor = Some(id.to_string());
        rows(database)
            .insert_one(
                doc! {"_id":format!("exception:{}:{}",control.run,control.counts.scanned),
                "document_schema_version":1_i64,"protocol":1_i64,"run":&control.run,
                "agent_id":id.as_str(),"reason":reason,"observed_at":control.updated_at,
                "fence":control.fence,"revision":control.revision},
            )
            .session(&mut session)
            .await?;
        persist(database, &before, &control, owner, &mut session).await
    }
    .await;
    if result.is_err() {
        let _ = session.abort_guarded().await;
        return Err(unavailable());
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(ProvenanceBackfillProgress::Advanced),
        _ => Err(unavailable()),
    }
}

#[cfg(test)]
pub(crate) async fn interrupt_for_test(store: &MongoSandboxStore, owner: &str) -> Result<()> {
    let database = store.client.database(&store.agents.namespace().db);
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| unavailable())?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable())?;
    let result = execute(store, &database, owner, &mut session).await;
    session.abort_guarded().await.map_err(|_| unavailable())?;
    result.map(|_| ()).map_err(|_| unavailable())
}
