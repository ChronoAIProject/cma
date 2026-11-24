//! Indexed detection and fenced recovery of pending Launch home reservations.
use super::{
    activity::{ActivityDoc, ACTIVITY},
    durability::majority_collection,
    launch_home_capacity::{fence_key, pending_homes},
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, require_version, start_transaction,
        CommitOutcome, TRANSACTION_ATTEMPTS,
    },
    schema::DOCUMENT_SCHEMA_VERSION,
    settings::SETTINGS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::{
    admission::{PendingHomesObservation, PendingHomesReconciliation},
    StoreError, SubjectId,
};
use mongodb::{
    bson::{doc, Document},
    options::Hint,
    Client, ClientSession, Collection,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

mod correction;
mod readiness;
mod repair;

pub(super) const INDEX: &str = "sandbox_admissions_reserved_launch_homes";
const QUERY_TIME: Duration = Duration::from_secs(3);
pub(crate) const BATCH_SIZE: i64 = 64;
const ADMISSIONS: &str = "sandbox_admissions";

pub(super) fn reserved_filter() -> Document {
    doc! {"_id.scope": "launch_home_capacity", "state": "reserved"}
}

#[derive(Clone)]
pub(crate) struct PendingLaunchHomes {
    enabled: bool,
    client: Client,
    admissions: Collection<Document>,
    launches: Collection<Document>,
    settings: Collection<Document>,
    activity: Collection<ActivityDoc>,
    activation: Arc<AtomicBool>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Observation {
    pub pending: u64,
    pub expected: u64,
    pub drift: i64,
}

#[derive(Debug, Default)]
pub(crate) struct SweepReport {
    pub scanned: u64,
    pub released: u64,
    pub missing_launches: u64,
    pub next: Option<Document>,
}

pub(super) enum Error {
    Driver(mongodb::error::Error),
    Domain(StoreError),
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<StoreError> for Error {
    fn from(value: StoreError) -> Self {
        Self::Domain(value)
    }
}

struct Correction<'a> {
    observed: u64,
    actor: &'a SubjectId,
    reason: &'a str,
    dry_run: bool,
    now: u64,
    id: &'a str,
}
enum Operation<'a> {
    Observe,
    Correct(Correction<'a>),
    Repair(&'a str),
}

enum Outcome {
    Observation(Observation),
    Correction(PendingHomesReconciliation),
    Repaired(bool),
}

impl PendingLaunchHomes {
    pub(super) fn new(client: &Client, database: &str) -> Self {
        let database = client.database(database);
        Self {
            enabled: false,
            client: client.clone(),
            admissions: majority_collection(&database, ADMISSIONS),
            launches: majority_collection(&database, "trigger_launches"),
            settings: majority_collection(&database, SETTINGS),
            activity: majority_collection(&database, ACTIVITY),
            activation: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn with_activation(mut self, activation: Arc<AtomicBool>) -> Self {
        self.activation = activation;
        self
    }

    pub(super) fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn active(&self) -> bool {
        self.activation.load(Ordering::Relaxed)
    }

    async fn require_activation(&self, session: &mut ClientSession) -> Result<(), Error> {
        if !self.enabled || !self.active() {
            return Err(unavailable("pending home recovery is disabled or inactive").into());
        }
        match super::registration_capacity::activation::read(&self.admissions, Some(session)).await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(unavailable("pending home activation disappeared").into()),
            Err(super::registration_capacity::activation::Error::Driver(error)) => {
                Err(Error::Driver(error))
            }
            Err(super::registration_capacity::activation::Error::Unreadable(error)) => {
                Err(Error::Domain(error))
            }
        }
    }

    /// Called inside the existing admin read-only snapshot, never a fence write.
    pub(super) async fn observe_in(
        &self,
        session: &mut ClientSession,
    ) -> Result<Observation, Error> {
        self.require_activation(session).await?;
        let expected = self
            .admissions
            .count_documents(reserved_filter())
            .hint(Hint::Name(INDEX.into()))
            .max_time(QUERY_TIME)
            .session(&mut *session)
            .await?;
        let row = self
            .admissions
            .find_one(fence_key())
            .max_time(QUERY_TIME)
            .session(session)
            .await?;
        let pending = pending_homes(row.as_ref())?;
        let drift = i64::try_from(pending)
            .ok()
            .zip(i64::try_from(expected).ok())
            .map(|(pending, expected)| pending - expected)
            .ok_or_else(|| unavailable("pending home observation overflow"))?;
        Ok(Observation {
            pending,
            expected,
            drift,
        })
    }

    pub(crate) async fn observe(&self) -> Result<Observation, StoreError> {
        self.require_ready().await?;
        match self.transaction(Operation::Observe).await? {
            Outcome::Observation(observation) => {
                report_drift(&observation);
                Ok(observation)
            }
            _ => unreachable!("observation transaction returns observation"),
        }
    }

    async fn transaction(&self, operation: Operation<'_>) -> Result<Outcome, StoreError> {
        for attempt in 0..TRANSACTION_ATTEMPTS {
            super::contention::backoff(attempt).await;
            let mut session =
                self.client.start_session().await.map_err(|error| {
                    unavailable(&format!("start pending home session: {error}"))
                })?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    continue;
                }
                return Err(unavailable(&format!(
                    "start pending home transaction: {error}"
                )));
            }
            let result = match &operation {
                Operation::Observe => self
                    .observe_in(&mut session)
                    .await
                    .map(Outcome::Observation),
                Operation::Correct(request) => self
                    .correct_in(request, &mut session)
                    .await
                    .map(Outcome::Correction),
                Operation::Repair(launch) => self
                    .repair_in(launch, &mut session)
                    .await
                    .map(Outcome::Repaired),
            };
            match result {
                Ok(outcome) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(outcome),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                        return Err(unavailable(&format!(
                            "finish pending home transaction: {error}"
                        )))
                    }
                },
                Err(Error::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(Error::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if is_transient(&error) || is_duplicate_key(&error) {
                        continue;
                    }
                    return Err(unavailable(&format!("pending home transaction: {error}")));
                }
            }
        }
        Err(unavailable(
            "pending home transaction exhausted the bounded retry budget",
        ))
    }
}

pub(super) fn report_drift(observation: &Observation) {
    if observation.drift != 0 {
        tracing::warn!(target: "cma::admission", event = "pending_launch_homes_drift",
            pending_launch_homes = observation.pending,
            expected_pending_launch_homes = observation.expected,
            pending_launch_homes_drift = observation.drift,
            "pending Launch home counter differs from reserved receipts");
    }
}
fn unavailable(message: &str) -> StoreError {
    StoreError::Unavailable(message.into())
}
