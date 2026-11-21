use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep};
use crate::seconds_from_rfc3339;

pub(super) fn validate(
    agent: &Agent,
    latest: Option<&(String, u64)>,
    entries: &[AgentLogEntry],
) -> Result<(), String> {
    match (
        agent.conversation.conversation_id.as_deref(),
        agent.conversation.runtime_generation,
        latest,
    ) {
        (None, None, None) => Ok(()),
        (Some(id), Some(generation), Some((logged_id, logged_generation)))
            if id == logged_id && generation == *logged_generation =>
        {
            Ok(())
        }
        // Initial opening durably saves the native identity and completion
        // receipt before PublishReady appends ConversationOpened. Readers can
        // observe that valid intermediate state, including after a restart.
        // It authorizes no turn and fabricates no conversation-available event.
        (Some(id), Some(1), None) if !id.trim().is_empty() && unpublished_open(agent, entries) => {
            Ok(())
        }
        _ => Err("agent conversation identity disagrees with its log".into()),
    }
}

fn unpublished_open(agent: &Agent, entries: &[AgentLogEntry]) -> bool {
    if agent.ready_at.is_some()
        || agent.conversation.active_turn.is_some()
        || agent.conversation.operation.is_some()
        || !matches!(
            agent.status,
            AgentStatus::Starting
                | AgentStatus::Failed
                | AgentStatus::Stopped
                | AgentStatus::Deleted
        )
        || (agent.status == AgentStatus::Starting && agent.work.step != AgentWorkStep::PublishReady)
    {
        return false;
    }
    let mut receipts = agent
        .work
        .receipts
        .iter()
        .filter(|receipt| receipt.step == AgentWorkStep::OpenConversation);
    let Some(receipt) = receipts.next() else {
        return false;
    };
    if receipts.next().is_some()
        || receipt.operation_id != format!("agent:{}:open:v1", agent.id)
        || receipt.request_sha256.len() != 64
        || !receipt
            .request_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || seconds_from_rfc3339(&receipt.completed_at).is_none()
    {
        return false;
    }
    let mut completed = entries.iter().filter(|entry| {
        matches!(
            entry.kind,
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::OpenConversation
            }
        )
    });
    completed
        .next()
        .is_some_and(|entry| entry.appended_at == receipt.completed_at)
        && completed.next().is_none()
}
