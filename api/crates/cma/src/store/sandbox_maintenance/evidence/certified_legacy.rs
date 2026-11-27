//! Transaction-local evidence for legacy histories above the normal scan cap.
use std::collections::BTreeMap;
use std::time::Duration;

use layer2_core::{
    sandbox::{Agent, AgentStatus, InactivityPauseRefusal, SandboxAgentLogActivity},
    AgentId,
};

use crate::store::agent_log_integrity::inactivity::{self, Preparation, Proof};

use super::{
    legacy_log_budget::MAX_LEGACY_BYTES, scoped::CapturedActivities, ApplyError, ClientSession,
    MongoSandboxStore,
};

pub(super) type CertifiedHeads = BTreeMap<AgentId, SandboxAgentLogActivity>;

pub(in crate::store::sandbox_maintenance) struct Unproven {
    pub(in crate::store::sandbox_maintenance) reason: InactivityPauseRefusal,
    pub(in crate::store::sandbox_maintenance) agents: Vec<Agent>,
}

pub(in crate::store::sandbox_maintenance) async fn capture(
    store: &MongoSandboxStore,
    agents: &[Agent],
    segmented: Option<&CapturedActivities>,
    budget_reason: InactivityPauseRefusal,
    session: &mut ClientSession,
) -> Result<Result<CertifiedHeads, Unproven>, ApplyError> {
    let mut heads = BTreeMap::new();
    let mut unproven = Vec::new();
    let mut preparing = false;
    let mut bytes_left = MAX_LEGACY_BYTES as usize;
    for agent in agents.iter().filter(|agent| agent.status.is_live()) {
        if segmented.is_some_and(|sources| sources.contains(agent)) {
            continue;
        }
        match inactivity::current(store, agent, &mut bytes_left, session).await? {
            Proof::Latest(entry) => {
                heads.insert(agent.id.clone(), entry);
            }
            Proof::Missing => unproven.push(agent.clone()),
            Proof::Preparing => {
                preparing = true;
                unproven.push(agent.clone());
            }
            Proof::Budget => {
                return Ok(Err(Unproven {
                    reason: InactivityPauseRefusal::LegacyLogProofByteLimit,
                    agents: Vec::new(),
                }))
            }
        }
    }
    if unproven.is_empty() {
        Ok(Ok(heads))
    } else {
        Ok(Err(Unproven {
            reason: if preparing {
                InactivityPauseRefusal::LegacyLogProofPending
            } else {
                budget_reason
            },
            agents: unproven,
        }))
    }
}

pub(in crate::store::sandbox_maintenance) async fn prepare_next(
    store: &MongoSandboxStore,
    unproven: &[Agent],
) {
    let Some(agent) = unproven.iter().find(|agent| {
        matches!(agent.status, AgentStatus::Ready | AgentStatus::Paused)
            && !agent.work.step.is_pending()
            && agent.work.lease.is_none()
            && agent.requested_stop_reason.is_none()
            && agent.conversation.active_turn.is_none()
            && agent.conversation.queued_messages.is_empty()
            && agent.conversation.last_seq > 0
    }) else {
        return;
    };
    let Ok(_permit) = store.inactivity_log_preparation.try_acquire() else {
        return;
    };
    let result =
        tokio::time::timeout(Duration::from_secs(5), inactivity::advance(store, agent)).await;
    let outcome = match result {
        Ok(Ok(Preparation::Pending)) => "progress",
        Ok(Ok(Preparation::Certified)) => "certified",
        Ok(Ok(Preparation::ByteLimit)) => "byte_limit",
        Ok(Err(_)) => "refused",
        Err(_) => "timeout",
    };
    metrics::counter!("cma_auto_pause_log_preparation_total", "outcome" => outcome).increment(1);
}
