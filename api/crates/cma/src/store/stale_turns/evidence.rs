use futures_util::TryStreamExt;
use layer2_core::{
    ids::AgentCommandId,
    sandbox::{
        agent_io::{AdmittedAgentCommand, AgentOwner, AgentResponseStatus, CommandDispatch},
        store::AgentMessageSnapshot,
        AgentOperationDelivery, AgentOperationKind, AgentStatus, Sandbox, SandboxStatus,
        SandboxWorkStep,
    },
    seconds_from_rfc3339, AgentId,
};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{Hint, ReadConcern},
};

use super::{scan::QUERY_DEADLINE, ProofBudget, ScanError, StaleTurnStore};
use crate::{
    sandbox::LiveAgentTarget,
    store::{
        agent_commands::records, agent_conversation_store::checkpoint, agents::read_agent,
        sandboxes::read_sandbox,
    },
};

pub(super) mod progress;
use progress::Progress;

const NO_PROGRESS_SECONDS: u64 = 15 * 60;
pub(super) const MAX_DOCUMENT_BYTES: i64 = 128 * 1024;
pub(super) const MAX_TURN_ROWS: u64 = progress::MAX_ROWS;

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct Candidate {
    pub command: AgentCommandId,
    agent: AgentId,
    turn: u64,
    generation: u64,
}

pub(crate) enum Observation {
    Progressing,
    Hold(&'static str),
    Quiet {
        target: LiveAgentTarget,
        age: u64,
        evidence: Box<QuietTurn>,
    },
}

pub(crate) struct QuietTurn {
    pub snapshot: AgentMessageSnapshot,
    pub parent: Sandbox,
    pub command: AdmittedAgentCommand,
}

impl Candidate {
    pub(crate) fn from_row(row: &Document, now: DateTime) -> Option<Self> {
        let record = row.get_document("record").ok()?;
        let envelope = record
            .get_document("receipt")
            .ok()?
            .get_document("envelope")
            .ok()?;
        if envelope
            .get_document("command")
            .ok()?
            .get_str("type")
            .ok()?
            != "message"
        {
            return None;
        }
        let dispatch = record
            .get_document("work")
            .ok()?
            .get_document("dispatch")
            .ok()?;
        let started = seconds_from_rfc3339(dispatch.get_str("fenced_at").ok()?)?;
        if clock_seconds(now).checked_sub(started)? < NO_PROGRESS_SECONDS {
            return None;
        }
        let turn = positive(dispatch, "turn_sequence")?;
        let generation = positive(dispatch, "runtime_generation")?;
        Some(Self {
            command: AgentCommandId(bounded_key(row, "_id")?),
            agent: AgentId(bounded_key(row, "agent_id")?),
            turn,
            generation,
        })
    }

    pub(crate) fn early_hold(row: &Document) -> Option<&'static str> {
        if row.get_str("status").ok() == Some("waiting_action") {
            return Some("approval_pending");
        }
        let work = row.get_document("record").ok()?.get_document("work").ok()?;
        let delivery = work
            .get_document("observation")
            .ok()
            .and_then(|observation| observation.get_str("delivery").ok())
            .or_else(|| work.get_document("dispatch").ok()?.get_str("delivery").ok());
        (!matches!(delivery, Some("write_confirmed" | "execution_observed")))
            .then_some("delivery_uncertain")
    }
}

impl StaleTurnStore {
    pub(crate) async fn observe(
        &self,
        candidate: &Candidate,
        now: DateTime,
        budget: &mut ProofBudget,
    ) -> Result<Observation, ScanError> {
        self.observe_checkpoint(candidate, now, budget, false).await
    }

    pub(crate) async fn observe_for_terminal(
        &self,
        candidate: &Candidate,
        now: DateTime,
        budget: &mut ProofBudget,
    ) -> Result<Observation, ScanError> {
        self.observe_checkpoint(candidate, now, budget, true).await
    }

    async fn observe_checkpoint(
        &self,
        candidate: &Candidate,
        now: DateTime,
        budget: &mut ProofBudget,
        read_existing_seal: bool,
    ) -> Result<Observation, ScanError> {
        let mut snapshot = self.store.client.start_session().snapshot(true).await?;
        let Some(command) = self
            .store
            .agent_commands
            .find_one(doc! {"_id": candidate.command.as_str()})
            .projection(budget.projection(MAX_DOCUMENT_BYTES))
            .read_concern(ReadConcern::snapshot())
            .session(&mut snapshot)
            .max_time(QUERY_DEADLINE)
            .await?
        else {
            return Ok(Observation::Hold("command_unavailable"));
        };
        let command = records::read(&budget.take_row(command)?).map_err(|_| ScanError::Invalid)?;
        if command.status.is_terminal() {
            return Ok(Observation::Hold("canonical_result"));
        }
        let envelope = &command.receipt.envelope;
        let Some(dispatch) = command.work.dispatch.as_ref() else {
            return Ok(Observation::Hold("delivery_uncertain"));
        };
        if envelope.agent_id != candidate.agent
            || envelope.command_id != candidate.command
            || envelope.command.kind() != AgentOperationKind::SendMessage
            || !candidate.matches_dispatch(dispatch)
        {
            return Ok(Observation::Hold("identity_changed"));
        }
        if command.status == AgentResponseStatus::WaitingAction {
            return Ok(Observation::Hold("approval_pending"));
        }
        if command.status != AgentResponseStatus::InProgress
            || !matches!(
                command
                    .work
                    .observation
                    .as_ref()
                    .map_or(dispatch.delivery, |value| value.delivery),
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            )
        {
            return Ok(Observation::Hold("delivery_uncertain"));
        }

        let Some(row) = self
            .store
            .agents
            .find_one(doc! {"_id": candidate.agent.as_str()})
            .projection(budget.projection(MAX_DOCUMENT_BYTES))
            .read_concern(ReadConcern::snapshot())
            .session(&mut snapshot)
            .max_time(QUERY_DEADLINE)
            .await?
        else {
            return Ok(Observation::Hold("agent_unavailable"));
        };
        let row = budget.take_row(row)?;
        let agent = read_agent(&row)?;
        let conversation = &agent.conversation;
        if conversation.active_turn != Some(candidate.turn)
            || conversation.runtime_generation != Some(candidate.generation)
            || !matches!(&envelope.owner, AgentOwner::Person { subject } if *subject == agent.owner)
        {
            return Ok(Observation::Hold("identity_changed"));
        }
        if agent.status == AgentStatus::WaitingApproval {
            return Ok(Observation::Hold("approval_pending"));
        }
        if agent.status != AgentStatus::Working
            || conversation.operation.as_ref().is_some_and(|operation| {
                operation.turn_sequence != candidate.turn
                    || operation.sequence != dispatch.operation_sequence
                    || operation.runtime_generation != candidate.generation
            })
        {
            return Ok(Observation::Hold("agent_control_pending"));
        }
        let Some(conversation_id) = &conversation.conversation_id else {
            return Ok(Observation::Hold("identity_changed"));
        };
        let Some(parent) = self
            .store
            .sandboxes
            .find_one(doc! {"_id": agent.sandbox_id.as_str()})
            .projection(budget.projection(MAX_DOCUMENT_BYTES))
            .read_concern(ReadConcern::snapshot())
            .session(&mut snapshot)
            .max_time(QUERY_DEADLINE)
            .await?
        else {
            return Ok(Observation::Hold("parent_unavailable"));
        };
        let parent = read_sandbox(&budget.take_row(parent)?)?;
        if parent.owner != agent.owner
            || parent.status != SandboxStatus::Active
            || parent.lifecycle.operation.is_some()
            || !matches!(
                parent.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
        {
            // Active parents normally retain an idle credential-rotation timer.
            return Ok(Observation::Hold("parent_control_pending"));
        }
        if parent
            .runtime
            .as_ref()
            .is_none_or(|runtime| runtime.boot_id.is_none())
        {
            return Ok(Observation::Hold("runtime_identity_unavailable"));
        }

        let checkpoint = checkpoint::read(&row)?;
        if let Some(checkpoint) = &checkpoint {
            if !checkpoint.matches(&agent)
                || (!checkpoint.assembly.is_empty()
                    && (!read_existing_seal || checkpoint.output.is_none()))
            {
                return Ok(Observation::Hold("output_uncommitted"));
            }
        }
        // Both formats retain the same Agent log rows. These projected fields
        // are diagnostic only and never stand in for the recorder's proof.
        let head = conversation.last_seq;
        if head
            .checked_sub(candidate.turn)
            .is_none_or(|length| length >= progress::MAX_ROWS)
        {
            return Ok(Observation::Hold("history_budget"));
        }
        let mut cursor = self
            .store
            .agent_log
            .find(doc! {"agent_id": candidate.agent.as_str(), "seq": {
                "$gte": i64::try_from(candidate.turn).map_err(|_| ScanError::Invalid)?,
                "$lte": i64::try_from(head).map_err(|_| ScanError::Invalid)?,
            }})
            .sort(doc! {"seq": 1})
            .hint(Hint::Name("agent_log_sequence".into()))
            .projection(progress::projection())
            .max_time(QUERY_DEADLINE)
            .limit(-(progress::MAX_ROWS as i64))
            .batch_size(progress::MAX_ROWS as u32)
            .read_concern(ReadConcern::snapshot())
            .session(&mut snapshot)
            .await?;
        let mut progress = Progress::new(candidate.turn, &candidate.command);
        let mut stream = cursor.stream(&mut snapshot);
        while let Some(row) = stream.try_next().await? {
            budget.charge_row(&row)?;
            progress.observe(&row)?;
        }
        let age = match progress.finish(head, clock_seconds(now)) {
            Ok(age) if age >= NO_PROGRESS_SECONDS => age,
            Ok(_) => return Ok(Observation::Progressing),
            Err(reason) => return Ok(Observation::Hold(reason)),
        };
        Ok(Observation::Quiet {
            target: LiveAgentTarget {
                agent_id: candidate.agent.clone(),
                conversation_id: conversation_id.clone(),
                runtime_generation: candidate.generation,
            },
            age,
            evidence: Box::new(QuietTurn {
                snapshot: AgentMessageSnapshot { agent, checkpoint },
                parent,
                command,
            }),
        })
    }
}

impl Candidate {
    fn matches_dispatch(&self, dispatch: &CommandDispatch) -> bool {
        dispatch.claim.command_id == self.command
            && dispatch.claim.agent_id == self.agent
            && dispatch.turn_sequence == self.turn
            && dispatch.runtime_generation == self.generation
    }
}

pub(super) fn positive(document: &Document, field: &str) -> Option<u64> {
    let value = match document.get(field)? {
        mongodb::bson::Bson::Int32(value) => i64::from(*value),
        mongodb::bson::Bson::Int64(value) => *value,
        _ => return None,
    };
    u64::try_from(value).ok().filter(|value| *value != 0)
}

fn bounded_key(row: &Document, field: &str) -> Option<String> {
    let value = row.get_str(field).ok()?;
    (!value.is_empty() && value.len() <= 256).then(|| value.to_owned())
}

fn clock_seconds(now: DateTime) -> u64 {
    u64::try_from(now.timestamp_millis().max(0)).unwrap_or(0) / 1000
}
