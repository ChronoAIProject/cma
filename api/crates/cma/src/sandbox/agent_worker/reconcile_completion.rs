//! Bounded native-history backstop for a lost ACP terminal response.

use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentMessageAssembly, AgentStatus, NativeCompletionEvidence, SandboxStatus,
};
use layer3_adapter::{AgentSlot, PromptResponse, TurnStopReason};
use sha2::{Digest, Sha256};

use layer2_core::{rfc3339_at, seconds_from_rfc3339, AgentId};

use super::AgentWorker;

const MAX_RECONCILIATION_LOG_ENTRIES: u64 = 4_096;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RepairReport {
    pub inspected: usize,
    pub eligible: usize,
    pub repaired: usize,
    pub held: usize,
}

#[path = "reconcile_history.rs"]
mod reconcile_history;
use reconcile_history::{parse_history, MAX_HISTORY_BYTES};
#[path = "reconcile_repair.rs"]
mod reconcile_repair;
pub(crate) use reconcile_repair::inspect_stale_turns_read_only;

impl AgentWorker {
    /// Rebuild native completion evidence at command time. The command is
    /// dry-run unless `apply` is true; both modes use the same identity and
    /// control-state fences, and apply delegates to the durable CAS recorder.
    pub(crate) async fn repair_stale_turns(
        &self,
        ids: &[AgentId],
        now: SystemTime,
        apply: bool,
    ) -> Result<RepairReport, SandboxStoreError> {
        let mut report = RepairReport::default();
        for id in ids.iter().take(64) {
            report.inspected += 1;
            let Some(agent) = self.dependencies.store.get_agent(id).await? else {
                report.held += 1;
                tracing::info!(agent_id = %id, "stale-turn repair held: agent not found");
                continue;
            };
            let Some((evidence, _hash)) = self.operator_evidence(&agent).await? else {
                report.held += 1;
                tracing::info!(agent_id = %id, "stale-turn repair held: evidence did not qualify");
                continue;
            };
            report.eligible += 1;
            if !apply {
                tracing::info!(
                    agent_id = %id,
                    generation = evidence.runtime_generation,
                    turn_sequence = evidence.turn_sequence,
                    history_sha256 = %evidence.history_sha256,
                    "stale-turn repair dry-run eligible"
                );
                continue;
            }
            let mut assembly = AgentMessageAssembly::default();
            if self
                .dependencies
                .updates
                .restore_assembly(id, evidence.runtime_generation, &mut assembly)
                .await
                .is_err()
            {
                report.held += 1;
                tracing::info!(agent_id = %id, "stale-turn repair held: checkpoint assembly unavailable");
                continue;
            }
            let response = PromptResponse::new(TurnStopReason::EndTurn);
            match self
                .dependencies
                .updates
                .reconcile_completion(
                    id,
                    evidence.runtime_generation,
                    &mut assembly,
                    evidence,
                    response,
                    rfc3339_at(now),
                )
                .await
            {
                Ok(outcome) if outcome.turn_closed => {
                    report.repaired += 1;
                    tracing::info!(agent_id = %id, "stale-turn repair applied");
                }
                Ok(_) => {
                    tracing::info!(agent_id = %id, "stale-turn repair already reconciled");
                }
                Err(_) => {
                    report.held += 1;
                    tracing::info!(agent_id = %id, "stale-turn repair held: CAS fence changed");
                }
            }
        }
        Ok(report)
    }

    async fn operator_evidence(
        &self,
        agent: &Agent,
    ) -> Result<Option<(NativeCompletionEvidence, String)>, SandboxStoreError> {
        let Some(turn_sequence) = agent.conversation.active_turn else {
            return Ok(None);
        };
        if agent.status != AgentStatus::Working || agent.requested_stop_reason.is_some() {
            return Ok(None);
        }
        let Some(generation) = agent.conversation.runtime_generation else {
            return Ok(None);
        };
        let Some(conversation) = agent.conversation.conversation_id.as_deref() else {
            return Ok(None);
        };
        let Some(sandbox) = self
            .dependencies
            .store
            .get_sandbox(&agent.sandbox_id)
            .await?
        else {
            return Ok(None);
        };
        if sandbox.status != SandboxStatus::Active {
            return Ok(None);
        }
        let Some(runtime) = sandbox.runtime.as_ref() else {
            return Ok(None);
        };
        let handle = layer3_adapter::RuntimeHandle::new(&runtime.runtime_id);
        let slot = AgentSlot::new(agent.id.as_str());
        let first = self
            .dependencies
            .adapter
            .export_agent_history(&handle, &slot)
            .await;
        let second = self
            .dependencies
            .adapter
            .export_agent_history(&handle, &slot)
            .await;
        let (Ok(first), Ok(second)) = (first, second) else {
            return Ok(None);
        };
        if first.len() > MAX_HISTORY_BYTES || first != second {
            return Ok(None);
        }
        let hash = format!("{:x}", Sha256::digest(&first));
        let Some(expected) = agent
            .conversation
            .observations
            .usage
            .as_ref()
            .filter(|usage| usage.conversation_id == conversation)
            .and_then(|usage| usage.current_turn)
            .filter(|turn| turn.turn_sequence == turn_sequence)
            .and(agent.conversation.observations.usage.as_ref())
            .map(|usage| usage.source_turn.as_str())
        else {
            return Ok(None);
        };
        let Some(native) = parse_history(&first, conversation, expected) else {
            return Ok(None);
        };
        let Some(start) = self
            .dependencies
            .store
            .agent_log_entry(&agent.id, turn_sequence)
            .await?
        else {
            return Ok(None);
        };
        if !matches!(start.kind, AgentLogKind::TurnStarted { .. }) {
            return Ok(None);
        }
        let Some(final_answer_durable) = self
            .durable_final_answer(agent, start.seq, &native.final_answer)
            .await?
        else {
            return Ok(None);
        };
        let evidence = NativeCompletionEvidence {
            native_conversation_id: native.conversation_id,
            runtime_generation: generation,
            turn_sequence,
            native_turn_id: native.turn_id,
            admitted_at: start.appended_at,
            completed_at: native.completed_at,
            task_started: true,
            task_complete: true,
            no_later_open_turn: true,
            history_complete: true,
            history_prefix_unchanged: true,
            history_sha256: hash.clone(),
            final_answer: native.final_answer,
            final_answer_durable,
            operation_sequence: agent
                .conversation
                .operation
                .as_ref()
                .map_or(0, |op| op.sequence),
            sandbox_revision: sandbox.revision,
            sandbox_status: sandbox.status,
        };
        if evidence.validate_against(agent).is_err() {
            return Ok(None);
        }
        Ok(Some((evidence, hash)))
    }

    async fn durable_final_answer(
        &self,
        agent: &Agent,
        start_seq: u64,
        answer: &str,
    ) -> Result<Option<bool>, SandboxStoreError> {
        if answer.len() > layer2_core::sandbox::MAX_DURABLE_AGENT_MESSAGE_BYTES
            || start_seq > agent.conversation.last_seq
        {
            return Ok(None);
        }
        let count = agent.conversation.last_seq - start_seq;
        if count > MAX_RECONCILIATION_LOG_ENTRIES {
            return Ok(None);
        }
        let expected = format!("{:x}", Sha256::digest(answer.as_bytes()));
        let mut after = start_seq;
        let mut messages = Vec::new();
        while after < agent.conversation.last_seq {
            let limit = (agent.conversation.last_seq - after)
                .min(u64::from(layer2_core::sandbox::store::MAX_PAGE_SIZE))
                as u16;
            let entries = self
                .dependencies
                .store
                .agent_log(&agent.id, after, limit)
                .await?;
            if entries.len() != usize::from(limit) {
                return Ok(None);
            }
            for entry in entries {
                if let AgentLogKind::AgentMessage { content } = entry.kind {
                    if format!("{:x}", Sha256::digest(content.as_bytes())) == expected {
                        return Ok(Some(true));
                    }
                    messages.push(content);
                }
                after = entry.seq;
            }
        }
        for start in 0..messages.len() {
            let mut joined = String::new();
            for message in messages.iter().skip(start) {
                if joined.len() >= answer.len() {
                    break;
                }
                joined.push_str(message);
                if joined == answer {
                    return Ok(Some(true));
                }
                if !answer.starts_with(&joined) {
                    break;
                }
            }
        }
        Ok(Some(false))
    }

    /// Inspect one live Agent at most once per sweep and reconcile only after
    /// two identical, complete native-history reads. Errors hold the Agent for
    /// a later bounded retry; they never become a synthetic failure.
    pub(super) async fn reconcile_native_completion(
        &self,
        agent: &Agent,
        now: SystemTime,
    ) -> Result<usize, SandboxStoreError> {
        let Some(turn_sequence) = agent.conversation.active_turn else {
            return Ok(0);
        };
        if agent.status != AgentStatus::Working || agent.requested_stop_reason.is_some() {
            return Ok(0);
        }
        let Some(operation) = agent.conversation.operation.as_ref().filter(|operation| {
            operation.turn_sequence == turn_sequence
                && operation.state
                    == layer2_core::sandbox::AgentOperationDelivery::ExecutionObserved
        }) else {
            return Ok(0);
        };
        let Some(observed_at) =
            seconds_from_rfc3339(&operation.execution_observed_at.clone().unwrap_or_default())
        else {
            return Ok(0);
        };
        let now_seconds = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        if now_seconds.saturating_sub(observed_at as i64) < MIN_RECONCILIATION_AGE_SECONDS {
            return Ok(0);
        }
        {
            let mut checked = self
                .completion_checked_at
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if checked
                .get(&agent.id)
                .and_then(|at| now.duration_since(*at).ok())
                .is_some_and(|elapsed| elapsed < COMPLETION_CHECK_COOLDOWN)
            {
                return Ok(0);
            }
            checked.insert(agent.id.clone(), now);
            checked.retain(|_, at| {
                now.duration_since(*at)
                    .map_or(true, |elapsed| elapsed < COMPLETION_CHECK_CACHE_TTL)
            });
        }
        let Some(generation) = agent.conversation.runtime_generation else {
            return Ok(0);
        };
        let Some(conversation) = agent.conversation.conversation_id.as_deref() else {
            return Ok(0);
        };
        let Some(sandbox) = self
            .dependencies
            .store
            .get_sandbox(&agent.sandbox_id)
            .await?
        else {
            return Ok(0);
        };
        if sandbox.status != SandboxStatus::Active {
            return Ok(0);
        }
        let Some(runtime) = sandbox.runtime.as_ref() else {
            return Ok(0);
        };
        let bytes = match self
            .dependencies
            .adapter
            .export_agent_history(
                &layer3_adapter::RuntimeHandle::new(&runtime.runtime_id),
                &AgentSlot::new(agent.id.as_str()),
            )
            .await
        {
            Ok(bytes) if bytes.len() <= MAX_HISTORY_BYTES => bytes,
            Ok(bytes) => return Ok(bytes.len()),
            Err(_) => {
                tracing::debug!(agent_id = %agent.id, "native completion evidence unavailable");
                return Ok(0);
            }
        };
        let bytes_read = bytes.len();
        let history_hash = format!("{:x}", Sha256::digest(&bytes));
        let key = (agent.id.clone(), generation);
        let unchanged = {
            let mut previous = self
                .completion_history
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let unchanged = previous
                .get(&key)
                .is_some_and(|(hash, _)| hash == &history_hash);
            previous.insert(key, (history_hash.clone(), now));
            if previous.len() > MAX_COMPLETION_HISTORY {
                if let Some(oldest) = previous
                    .iter()
                    .min_by_key(|(_, (_, checked_at))| *checked_at)
                    .map(|(key, _)| key.clone())
                {
                    previous.remove(&oldest);
                }
            }
            unchanged
        };
        if !unchanged {
            return Ok(bytes_read);
        }
        let Some(expected) = agent
            .conversation
            .observations
            .usage
            .as_ref()
            .filter(|usage| usage.conversation_id == conversation)
            .and_then(|usage| usage.current_turn)
            .filter(|turn| turn.turn_sequence == turn_sequence)
            .and(agent.conversation.observations.usage.as_ref())
            .map(|usage| usage.source_turn.as_str())
        else {
            return Ok(bytes_read);
        };
        let Some(native) = parse_history(&bytes, conversation, expected) else {
            return Ok(bytes_read);
        };
        let Some(start) = self
            .dependencies
            .store
            .agent_log_entry(&agent.id, turn_sequence)
            .await?
        else {
            return Ok(bytes_read);
        };
        let AgentLogKind::TurnStarted { .. } = start.kind else {
            return Ok(bytes_read);
        };
        let Some(final_answer_durable) = self
            .durable_final_answer(agent, start.seq, &native.final_answer)
            .await?
        else {
            return Ok(bytes_read);
        };
        let mut assembly = AgentMessageAssembly::default();
        if self
            .dependencies
            .updates
            .restore_assembly(&agent.id, generation, &mut assembly)
            .await
            .is_err()
        {
            return Ok(bytes_read);
        }
        let evidence = NativeCompletionEvidence {
            native_conversation_id: native.conversation_id,
            runtime_generation: generation,
            turn_sequence,
            native_turn_id: native.turn_id,
            admitted_at: start.appended_at,
            completed_at: native.completed_at,
            task_started: true,
            task_complete: true,
            no_later_open_turn: true,
            history_complete: true,
            history_prefix_unchanged: true,
            history_sha256: history_hash,
            final_answer: native.final_answer,
            final_answer_durable,
            operation_sequence: agent
                .conversation
                .operation
                .as_ref()
                .map_or(0, |op| op.sequence),
            sandbox_revision: sandbox.revision,
            sandbox_status: sandbox.status,
        };
        let response = PromptResponse::new(TurnStopReason::EndTurn);
        match self
            .dependencies
            .updates
            .reconcile_completion(
                &agent.id,
                generation,
                &mut assembly,
                evidence,
                response,
                rfc3339_at(now),
            )
            .await
        {
            Ok(outcome) => tracing::info!(
                agent_id = %agent.id,
                generation,
                turn_sequence,
                turn_closed = outcome.turn_closed,
                "native completion reconciliation completed"
            ),
            Err(_) => tracing::debug!(
                agent_id = %agent.id,
                generation,
                turn_sequence,
                "native completion reconciliation held"
            ),
        }
        Ok(bytes_read)
    }
}

const MIN_RECONCILIATION_AGE_SECONDS: i64 = 60;
const COMPLETION_CHECK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
const COMPLETION_CHECK_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);
const MAX_COMPLETION_HISTORY: usize = 4_096;
