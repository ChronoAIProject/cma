//! Durable custody for one exact whole-runtime retirement.
use layer2_core::sandbox::store::{retirement_debt_is_local, SandboxStoreError, WorkAuthority};
use layer2_core::sandbox::{Agent, AgentCleanupDebt, RuntimeBinding, Sandbox, SandboxHome};
use layer2_core::{AgentId, SandboxId, SubjectId};
use layer3_adapter::receiver_failure::RetirementReceiver;
use mongodb::{
    bson::{doc, Document},
    options::ReadConcern,
    ClientSession,
};
use serde::{Deserialize, Serialize};

use super::lifecycle_execution::{load_sandbox_agents, LifecycleLoadError};
use super::sandbox_mongo::{
    commit_transaction, is_transient, record_bson, signed, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::read_sandbox;

pub(super) const FIELD: &str = "native_retirement_action_v1";
pub(crate) mod capture;
mod child_write;
mod parent_write;
pub(super) use parent_write::preserve_custody;
mod reconciliation;
pub(super) use child_write::{adds_obligation, fence_obligation, insert_child};

pub(super) const OBSERVATION: &str = "native_retirement_receiver_v1";
const CHILD_WRITE_FENCE: &str = "native_retirement_writer_fence";
const WORKSPACE_ADMISSION_FENCE: &str = "workspace_admission_fence";

pub(crate) type RetirementFences = [Option<i64>; 2];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetirementObservation {
    pub owner: SubjectId,
    pub binding: RuntimeBinding,
    pub receiver: RetirementReceiver,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementPhase {
    Admitted,
    // Dispatch is authorized. Its result may still be unknown or not yet sent.
    Dispatching,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetirementAction {
    pub id: String,
    pub sandbox: SandboxId,
    pub observation: RetirementObservation,
    pub home: Box<SandboxHome>,
    pub phase: RetirementPhase,
    children: Vec<RetirementChild>,
    writer_fences: RetirementFences,
}

#[async_trait::async_trait]
pub(crate) trait RetirementRecords: Send + Sync {
    async fn note(
        &self,
        parent: &Sandbox,
        observation: &RetirementObservation,
    ) -> Result<(), SandboxStoreError>;
    async fn observation(
        &self,
        id: &SandboxId,
    ) -> Result<Option<(RetirementObservation, RetirementFences)>, SandboxStoreError>;
    async fn action(&self, id: &SandboxId) -> Result<Option<RetirementAction>, SandboxStoreError>;
    async fn admit(
        &self,
        parent: &Sandbox,
        action: &RetirementAction,
    ) -> Result<(), SandboxStoreError>;
    async fn transition(
        &self,
        parent: &Sandbox,
        previous: &RetirementAction,
        next: Option<&RetirementAction>,
    ) -> Result<(), SandboxStoreError>;
    async fn settle(
        &self,
        request: layer2_core::sandbox::store::SandboxRuntimeLossRequest,
        action: &RetirementAction,
    ) -> Result<layer2_core::sandbox::store::SandboxRuntimeLossOutcome, SandboxStoreError>;
}

#[async_trait::async_trait]
impl RetirementRecords for MongoSandboxStore {
    async fn note(
        &self,
        parent: &Sandbox,
        observation: &RetirementObservation,
    ) -> Result<(), SandboxStoreError> {
        self.note_retirement_receiver(parent, observation).await
    }
    async fn observation(
        &self,
        id: &SandboxId,
    ) -> Result<Option<(RetirementObservation, RetirementFences)>, SandboxStoreError> {
        self.retirement_observation(id).await
    }
    async fn action(&self, id: &SandboxId) -> Result<Option<RetirementAction>, SandboxStoreError> {
        self.retirement_action(id).await
    }
    async fn admit(
        &self,
        parent: &Sandbox,
        action: &RetirementAction,
    ) -> Result<(), SandboxStoreError> {
        transact(self, parent, None, Some(action), None).await
    }
    async fn transition(
        &self,
        parent: &Sandbox,
        previous: &RetirementAction,
        next: Option<&RetirementAction>,
    ) -> Result<(), SandboxStoreError> {
        let valid = previous.phase == RetirementPhase::Admitted
            && match next {
                Some(next) => {
                    let mut dispatched = previous.clone();
                    dispatched.phase = RetirementPhase::Dispatching;
                    dispatched == *next
                }
                None => true,
            };
        if !valid {
            return Err(SandboxStoreError::Conflict);
        }
        transact(self, parent, Some(previous), next, None).await
    }
    async fn settle(
        &self,
        request: layer2_core::sandbox::store::SandboxRuntimeLossRequest,
        action: &RetirementAction,
    ) -> Result<layer2_core::sandbox::store::SandboxRuntimeLossOutcome, SandboxStoreError> {
        self.settle_retirement_action(request, action).await
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementChild {
    id: AgentId,
    debt: Vec<AgentCleanupDebt>,
}

impl RetirementAction {
    pub(crate) fn new(
        parent: &Sandbox,
        observation: RetirementObservation,
        children: &[Agent],
        writer_fences: RetirementFences,
    ) -> Result<Self, SandboxStoreError> {
        let action = Self {
            id: uuid::Uuid::new_v4().to_string(),
            sandbox: parent.id.clone(),
            observation,
            home: parent
                .lifecycle
                .home
                .clone()
                .ok_or(SandboxStoreError::Conflict)?,
            phase: RetirementPhase::Admitted,
            children: participants(parent, children)?,
            writer_fences,
        };
        if !action.matches(parent)
            || !action
                .children
                .iter()
                .any(|child| child.id.as_str() == action.observation.receiver.owner.agent_id)
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(action)
    }

    pub(crate) fn matches(&self, parent: &Sandbox) -> bool {
        self.sandbox == parent.id
            && self.observation.matches(parent)
            && parent.lifecycle.home.as_ref() == Some(&self.home)
    }

    pub(crate) fn covers(
        &self,
        parent: &Sandbox,
        agents: &[Agent],
    ) -> Result<bool, SandboxStoreError> {
        let current = participants(parent, agents)?;
        Ok(current.len() == self.children.len()
            && current
                .iter()
                .zip(&self.children)
                .all(|(current, admitted)| {
                    current.id == admitted.id
                        && current.debt.iter().all(|debt| admitted.debt.contains(debt))
                }))
    }
}

impl RetirementObservation {
    pub(crate) fn matches(&self, parent: &Sandbox) -> bool {
        parent.status == layer2_core::sandbox::SandboxStatus::Retiring
            && parent.work.step == layer2_core::sandbox::SandboxWorkStep::StopAgents
            && self.owner == parent.owner
            && Some(&self.binding) == parent.runtime.as_ref()
            && self.receiver.runtime.as_str() == self.binding.runtime_id
            && self.receiver.owner.validate().is_ok()
    }
}

fn participants(
    parent: &Sandbox,
    agents: &[Agent],
) -> Result<Vec<RetirementChild>, SandboxStoreError> {
    if agents.len() > layer2_core::sandbox::store::MAX_SANDBOX_LIFECYCLE_AGENTS
        || agents.iter().any(|agent| {
            agent.sandbox_id != parent.id
                || agent.owner != parent.owner
                || !retirement_debt_is_local(agent)
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut children = agents
        .iter()
        .map(|agent| RetirementChild {
            id: agent.id.clone(),
            debt: agent.work.cleanup_debt.clone(),
        })
        .collect::<Vec<_>>();
    children.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    if children.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(children)
}

impl MongoSandboxStore {
    pub(crate) async fn note_retirement_receiver(
        &self,
        parent: &Sandbox,
        observation: &RetirementObservation,
    ) -> Result<(), SandboxStoreError> {
        WorkAuthority::current()?;
        if !observation.matches(parent) {
            return Err(SandboxStoreError::Conflict);
        }
        let stored = record_bson(observation).map_err(SandboxStoreError::Corrupt)?;
        let result = self.sandboxes.update_one(
            super::work_schema::query(doc! {
                "_id": parent.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "record.owner": parent.owner.as_str(),
                "record.status": record_bson(&parent.status).map_err(SandboxStoreError::Corrupt)?,
                "record.work.step": record_bson(&parent.work.step).map_err(SandboxStoreError::Corrupt)?,
                "record.runtime": record_bson(&observation.binding).map_err(SandboxStoreError::Corrupt)?,
                FIELD: {"$exists":false},
            }),
            doc! {"$set":{OBSERVATION: stored}},
        ).await.map_err(unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        }
    }

    pub(crate) async fn retirement_observation(
        &self,
        id: &SandboxId,
    ) -> Result<Option<(RetirementObservation, RetirementFences)>, SandboxStoreError> {
        let row = self
            .sandboxes
            .find_one(doc! {"_id": id.as_str()})
            .projection(doc! {OBSERVATION:1, CHILD_WRITE_FENCE:1, WORKSPACE_ADMISSION_FENCE:1})
            .read_concern(ReadConcern::majority())
            .await
            .map_err(unavailable)?;
        let Some(row) = row else { return Ok(None) };
        observation_from(&row)?
            .map(|observation| writer_fences(&row).map(|fences| (observation, fences)))
            .transpose()
    }

    pub(crate) async fn retirement_action(
        &self,
        id: &SandboxId,
    ) -> Result<Option<RetirementAction>, SandboxStoreError> {
        let row = self
            .sandboxes
            .find_one(doc! {"_id": id.as_str()})
            .projection(doc! {FIELD:1})
            .read_concern(ReadConcern::majority())
            .await
            .map_err(unavailable)?;
        row.as_ref()
            .map(action_from)
            .transpose()
            .map(Option::flatten)
    }
}

pub(super) fn action_from(row: &Document) -> Result<Option<RetirementAction>, SandboxStoreError> {
    row.get(FIELD)
        .cloned()
        .map(|action| {
            mongodb::bson::deserialize_from_bson(action).map_err(|_| {
                SandboxStoreError::Corrupt("native retirement action is invalid".into())
            })
        })
        .transpose()
}

fn observation_from(row: &Document) -> Result<Option<RetirementObservation>, SandboxStoreError> {
    row.get(OBSERVATION)
        .cloned()
        .map(|observation| {
            mongodb::bson::deserialize_from_bson(observation).map_err(|_| {
                SandboxStoreError::Corrupt("native retirement receiver is invalid".into())
            })
        })
        .transpose()
}

fn writer_fences(row: &Document) -> Result<RetirementFences, SandboxStoreError> {
    let mut fences = [None, None];
    for (index, key) in [CHILD_WRITE_FENCE, WORKSPACE_ADMISSION_FENCE]
        .into_iter()
        .enumerate()
    {
        fences[index] = match row.get(key) {
            None => None,
            Some(mongodb::bson::Bson::Int64(value)) if *value >= 0 => Some(*value),
            Some(mongodb::bson::Bson::Int32(value)) if *value >= 0 => Some(i64::from(*value)),
            Some(_) => {
                return Err(SandboxStoreError::Corrupt(
                    "retirement writer fence is invalid".into(),
                ))
            }
        };
    }
    Ok(fences)
}

async fn transact(
    store: &MongoSandboxStore,
    parent: &Sandbox,
    previous: Option<&RetirementAction>,
    next: Option<&RetirementAction>,
    captured: Option<&capture::CapturedRetirementObservation>,
) -> Result<(), SandboxStoreError> {
    WorkAuthority::current()?;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let result = transaction(store, parent, previous, next, captured, &mut session).await;
        match result {
            Ok(()) => {}
            Err(LifecycleLoadError::Driver(error)) if retryable_body_error(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(match error {
                    LifecycleLoadError::Driver(error) => unavailable(error),
                    LifecycleLoadError::Domain(error) => error,
                });
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(error) => {
                return reconciliation::prove_commit(
                    store,
                    &parent.id,
                    next,
                    session.operation_time(),
                    error,
                )
                .await;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(SandboxStoreError::Conflict)
}

async fn transaction(
    store: &MongoSandboxStore,
    parent: &Sandbox,
    previous: Option<&RetirementAction>,
    next: Option<&RetirementAction>,
    captured: Option<&capture::CapturedRetirementObservation>,
    session: &mut ClientSession,
) -> Result<(), LifecycleLoadError> {
    use LifecycleLoadError::{Domain, Driver};
    start_transaction(session).await.map_err(Driver)?;
    let mut filter = super::work_schema::query(doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(parent.revision, "retirement parent revision")
            .map_err(|_| Domain(SandboxStoreError::Conflict))?,
        "$and": [super::work_claim_authority::sandbox_claim(parent).map_err(Domain)?],
    });
    let row = store
        .sandboxes
        .find_one(filter.clone())
        .session(&mut *session)
        .await
        .map_err(Driver)?
        .ok_or(Domain(SandboxStoreError::Conflict))?;
    let current = read_sandbox(&row).map_err(Domain)?;
    if current.id != parent.id
        || current.runtime != parent.runtime
        || current.owner != parent.owner
        || current.status != parent.status
        || current.lifecycle != parent.lifecycle
        || action_from(&row).map_err(Domain)?.as_ref() != previous
    {
        return Err(Domain(SandboxStoreError::Conflict));
    }
    if let Some(action) = next {
        let first = previous.is_none();
        let agents = load_sandbox_agents(store, &parent.id, session).await?;
        if !action.matches(parent)
            || (first
                && captured.is_none()
                && observation_from(&row).map_err(Domain)?.as_ref() != Some(&action.observation))
            || (first && writer_fences(&row).map_err(Domain)? != action.writer_fences)
            || (first && participants(parent, &agents).map_err(Domain)? != action.children)
            || (!first && !action.covers(parent, &agents).map_err(Domain)?)
        {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        if let Some(captured) = captured {
            captured
                .fence_admission(store, &row, &current, &agents, action, session)
                .await?;
        }
    }
    filter.insert(
        FIELD,
        previous
            .map_or_else(
                || Ok(mongodb::bson::Bson::Document(doc! {"$exists":false})),
                encoded,
            )
            .map_err(Domain)?,
    );
    let change = match next {
        Some(action) => doc! {"$set": {FIELD: encoded(action).map_err(Domain)?}},
        None => doc! {"$unset": {FIELD: ""}},
    };
    let updated = store
        .sandboxes
        .update_one(filter, change)
        .session(&mut *session)
        .await
        .map_err(Driver)?;
    if updated.matched_count == 1 {
        Ok(())
    } else {
        Err(Domain(SandboxStoreError::Conflict))
    }
}

fn encoded(action: &RetirementAction) -> Result<mongodb::bson::Bson, SandboxStoreError> {
    record_bson(action).map_err(SandboxStoreError::Corrupt)
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

pub(super) fn retryable_body_error(error: &mongodb::error::Error) -> bool {
    !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT) && is_transient(error)
}
