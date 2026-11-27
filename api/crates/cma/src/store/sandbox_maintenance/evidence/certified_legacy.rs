//! Transaction-local evidence for legacy histories above the normal scan cap.
use std::collections::BTreeMap;
use std::time::Duration;

use layer2_core::{
    sandbox::{Agent, AgentStatus, InactivityPauseRefusal, SandboxAgentLogActivity},
    AgentId,
};

use crate::store::agent_log_integrity::inactivity::{self, Preparation, Proof};

use super::{scoped::CapturedActivities, ApplyError, ClientSession, MongoSandboxStore};

pub(super) type CertifiedHeads = BTreeMap<AgentId, SandboxAgentLogActivity>;

pub(in crate::store::sandbox_maintenance) struct Unproven {
    pub(in crate::store::sandbox_maintenance) reason: InactivityPauseRefusal,
    pub(in crate::store::sandbox_maintenance) next_agent: Option<Box<Agent>>,
}

pub(in crate::store::sandbox_maintenance) async fn capture(
    store: &MongoSandboxStore,
    agents: &[Agent],
    segmented: Option<&CapturedActivities>,
    budget_reason: InactivityPauseRefusal,
    session: &mut ClientSession,
) -> Result<Result<CertifiedHeads, Unproven>, ApplyError> {
    // Validate at most one raw tail per live legacy Agent, with one 4 MiB
    // allowance per tail. Keep only the content-free projection between reads.
    // The sandbox-wide deadline bounds this work independently of Agent count.
    tokio::time::timeout(
        Duration::from_secs(5),
        capture_heads(store, agents, segmented, budget_reason, session),
    )
    .await
    .unwrap_or_else(|_| {
        Ok(Err(Unproven {
            reason: InactivityPauseRefusal::LegacyLogProofTimeout,
            next_agent: None,
        }))
    })
}

async fn capture_heads(
    store: &MongoSandboxStore,
    agents: &[Agent],
    segmented: Option<&CapturedActivities>,
    budget_reason: InactivityPauseRefusal,
    session: &mut ClientSession,
) -> Result<Result<CertifiedHeads, Unproven>, ApplyError> {
    let mut heads = BTreeMap::new();
    let mut missing = false;
    let mut next_agent = None;
    let mut preparing = false;
    for agent in agents.iter().filter(|agent| agent.status.is_live()) {
        if segmented.is_some_and(|sources| sources.contains(agent)) {
            continue;
        }
        let proof = inactivity::current(store, agent, session).await?;
        match proof {
            Proof::Latest(entry) => {
                heads.insert(agent.id.clone(), entry);
            }
            Proof::Missing | Proof::Preparing => {
                missing = true;
                preparing |= matches!(proof, Proof::Preparing);
                if next_agent.is_none() && can_prepare(agent) {
                    next_agent = Some(Box::new(agent.clone()));
                }
            }
            Proof::Budget => {
                return Ok(Err(Unproven {
                    reason: InactivityPauseRefusal::LegacyLogProofByteLimit,
                    next_agent: None,
                }))
            }
        }
    }
    if !missing {
        Ok(Ok(heads))
    } else {
        Ok(Err(Unproven {
            reason: if preparing {
                InactivityPauseRefusal::LegacyLogProofPending
            } else {
                budget_reason
            },
            next_agent,
        }))
    }
}

pub(in crate::store::sandbox_maintenance) async fn prepare_next(
    store: &MongoSandboxStore,
    agent: &Agent,
) {
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

fn can_prepare(agent: &Agent) -> bool {
    matches!(agent.status, AgentStatus::Ready | AgentStatus::Paused)
        && !agent.work.step.is_pending()
        && agent.work.lease.is_none()
        && agent.requested_stop_reason.is_none()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.queued_messages.is_empty()
        && agent.conversation.last_seq > 0
}
