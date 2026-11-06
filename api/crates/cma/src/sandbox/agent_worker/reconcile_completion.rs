//! Bounded native-history backstop for a lost ACP terminal response.

use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentMessageAssembly, AgentStatus, NativeCompletionEvidence, SandboxStatus,
};
use layer3_adapter::{AgentSlot, PromptResponse, TurnStopReason};
use serde_json::Value;
use sha2::{Digest, Sha256};

use layer2_core::{rfc3339_at, seconds_from_rfc3339, AgentId};

use super::AgentWorker;

const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_HISTORY_LINES: usize = 131_072;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RepairReport {
    pub inspected: usize,
    pub eligible: usize,
    pub repaired: usize,
    pub held: usize,
}

#[derive(Debug)]
struct NativeTerminal {
    turn_id: String,
    started_at: String,
    completed_at: String,
}

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
        if !matches!(
            agent.status,
            AgentStatus::Working | AgentStatus::WaitingApproval
        ) || agent.requested_stop_reason.is_some()
        {
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
            .and_then(|_| agent.conversation.observations.usage.as_ref())
            .map(|usage| usage.source_turn.as_str())
        else {
            return Ok(None);
        };
        let Some(native) = parse_history(&first, expected) else {
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
        let evidence = NativeCompletionEvidence {
            native_conversation_id: conversation.to_owned(),
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
        };
        if evidence.validate_against(agent).is_err() {
            return Ok(None);
        }
        Ok(Some((evidence, hash)))
    }

    /// Inspect one live Agent at most once per sweep and reconcile only after
    /// two identical, complete native-history reads. Errors hold the Agent for
    /// a later bounded retry; they never become a synthetic failure.
    pub(super) async fn reconcile_native_completion(
        &self,
        agent: &Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(turn_sequence) = agent.conversation.active_turn else {
            return Ok(());
        };
        if !matches!(
            agent.status,
            AgentStatus::Working | AgentStatus::WaitingApproval
        ) || agent.requested_stop_reason.is_some()
        {
            return Ok(());
        }
        let Some(generation) = agent.conversation.runtime_generation else {
            return Ok(());
        };
        let Some(conversation) = agent.conversation.conversation_id.as_deref() else {
            return Ok(());
        };
        let Some(sandbox) = self
            .dependencies
            .store
            .get_sandbox(&agent.sandbox_id)
            .await?
        else {
            return Ok(());
        };
        if sandbox.status != SandboxStatus::Active {
            return Ok(());
        }
        let Some(runtime) = sandbox.runtime.as_ref() else {
            return Ok(());
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
            Ok(_) => return Ok(()),
            Err(_) => {
                tracing::debug!(agent_id = %agent.id, "native completion evidence unavailable");
                return Ok(());
            }
        };
        let history_hash = format!("{:x}", Sha256::digest(&bytes));
        let key = (agent.id.clone(), generation);
        let unchanged = {
            let mut previous = self
                .completion_history
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let unchanged = previous.get(&key) == Some(&history_hash);
            previous.insert(key, history_hash.clone());
            unchanged
        };
        if !unchanged {
            return Ok(());
        }
        let Some(expected) = agent
            .conversation
            .observations
            .usage
            .as_ref()
            .filter(|usage| usage.conversation_id == conversation)
            .and_then(|usage| usage.current_turn)
            .filter(|turn| turn.turn_sequence == turn_sequence)
            .and_then(|_| agent.conversation.observations.usage.as_ref())
            .map(|usage| usage.source_turn.as_str())
        else {
            return Ok(());
        };
        let Some(native) = parse_history(&bytes, expected) else {
            return Ok(());
        };
        let Some(start) = self
            .dependencies
            .store
            .agent_log_entry(&agent.id, turn_sequence)
            .await?
        else {
            return Ok(());
        };
        let AgentLogKind::TurnStarted { .. } = start.kind else {
            return Ok(());
        };
        let mut assembly = AgentMessageAssembly::default();
        if self
            .dependencies
            .updates
            .restore_assembly(&agent.id, generation, &mut assembly)
            .await
            .is_err()
        {
            return Ok(());
        }
        let evidence = NativeCompletionEvidence {
            native_conversation_id: conversation.to_owned(),
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
        Ok(())
    }
}

fn parse_history(bytes: &[u8], expected: &str) -> Option<NativeTerminal> {
    if bytes.is_empty() || bytes.len() > MAX_HISTORY_BYTES {
        return None;
    }
    let mut open = std::collections::BTreeMap::<String, String>::new();
    let mut completed = None;
    let mut lines = 0;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        lines += 1;
        if lines > MAX_HISTORY_LINES {
            return None;
        }
        let value: Value = serde_json::from_slice(line).ok()?;
        let kind = value
            .get("kind")
            .or_else(|| value.get("event"))
            .and_then(Value::as_str)?;
        let turn = value
            .get("turn_id")
            .or_else(|| value.get("turnId"))
            .or_else(|| value.get("turn_sha256"))
            .or_else(|| value.get("turnSha256"))
            .and_then(Value::as_str)?
            .to_owned();
        let at = value.get("at").and_then(Value::as_str)?.to_owned();
        match kind {
            "task_started" => {
                open.insert(turn, at);
            }
            "task_complete" => {
                let Some(started_at) = open.remove(&turn) else {
                    continue;
                };
                if turn == expected {
                    completed = Some(NativeTerminal {
                        turn_id: turn,
                        started_at,
                        completed_at: at,
                    });
                }
            }
            _ => {}
        }
    }
    let terminal = completed?;
    let started = seconds_from_rfc3339(&terminal.started_at)?;
    let finished = seconds_from_rfc3339(&terminal.completed_at)?;
    (finished >= started && open.is_empty()).then_some(terminal)
}

use layer2_core::sandbox::AgentLogKind;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_requires_matching_complete_and_no_open_turn() {
        assert!(parse_history(
            br#"{"kind":"task_started","turn_id":"turn-a","at":"2026-01-01T00:00:00Z"}
{"kind":"task_complete","turn_id":"turn-a","at":"2026-01-01T00:00:01Z"}
"#,
            "turn-a"
        )
        .is_some());
        assert!(parse_history(
            br#"{"kind":"task_started","turn_id":"turn-a","at":"2026-01-01T00:00:00Z"}
{"kind":"task_complete","turn_id":"turn-a","at":"2026-01-01T00:00:01Z"}
{"kind":"task_started","turn_id":"turn-b","at":"2026-01-01T00:00:02Z"}
"#,
            "turn-a"
        )
        .is_none());
    }
}
