use super::*;
use layer3_adapter::{AgentProcessAction, AgentRestartRuntime, RuntimeStatusRequest};

#[test]
fn tool_open_requires_dispatched_successor_or_completed_receipt() {
    let mut state = snapshot();
    let lineage = &mut state.resource.lineage;
    let tree = lineage.worktree.as_mut().unwrap();
    tree.status = WorktreeStatus::Ready;
    let agent = lineage.agent.as_mut().unwrap();
    agent.work.step = AgentWorkStep::RestartConversation;
    agent.conversation.conversation_id = Some("original-native".into());
    agent.conversation.runtime_generation = Some(1);
    let operation = AgentProcessAction::Open.operation_id(agent.id.as_str(), 2);
    let mut open = AgentOpenRequest::new(
        operation,
        crate::sandbox::runtime_options(agent, &lineage.sandbox, tree),
        agent.conversation.conversation_id.clone(),
    );
    let runtime = lineage.sandbox.runtime.as_ref().unwrap();
    open.restart_runtime = Some(AgentRestartRuntime {
        boot_id: runtime.boot_id.clone().unwrap(),
        runtime: RuntimeStatusRequest {
            sandbox_id: lineage.sandbox.id.to_string(),
            generation: runtime.provision_generation.unwrap(),
            home_identity: lineage
                .sandbox
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .attachment()
                .unwrap()
                .identity()
                .unwrap(),
        },
    });
    assert!(validation::request(&state, &open, 1).is_err());
    let lineage = &mut state.resource.lineage;
    lineage
        .agent
        .as_mut()
        .unwrap()
        .work
        .cleanup_debt
        .push(AgentCleanupDebt {
            step: AgentWorkStep::OpenConversation,
            operation_id: open.operation_id.clone(),
            request_sha256: open.request_sha256.clone(),
        });
    assert!(validation::request(&state, &open, 1).is_ok());
    assert!(validation::request(&state, &open, 2).is_err());
    assert!(!crate::store::agent_tool_open::published(
        &state.resource.lineage,
        &open
    ));

    let mut other = open.clone();
    other.restart_runtime.as_mut().unwrap().boot_id = "c".repeat(32);
    assert!(validation::request(&state, &other, 1).is_err());
    let mut other = open.clone();
    other
        .restart_runtime
        .as_mut()
        .unwrap()
        .runtime
        .home_identity = "d".repeat(64);
    assert!(validation::request(&state, &other, 1).is_err());
    let mut other = open.clone();
    other.resume_conversation = Some("another-native".into());
    other.request_sha256 = other.computed_sha256();
    assert!(validation::request(&state, &other, 1).is_err());

    let lineage = &mut state.resource.lineage;
    let agent = lineage.agent.as_mut().unwrap();
    agent.work.step = AgentWorkStep::None;
    agent.work.lease = None;
    agent.work.cleanup_debt.clear();
    agent.conversation.runtime_generation = Some(2);
    agent.status = AgentStatus::Ready;
    assert!(!crate::store::agent_tool_open::published(lineage, &open));
    lineage
        .agent
        .as_mut()
        .unwrap()
        .work
        .receipts
        .push(AgentWorkReceipt {
            step: AgentWorkStep::OpenConversation,
            operation_id: open.operation_id.clone(),
            request_sha256: open.request_sha256.clone(),
            completed_at: NOW.into(),
        });
    assert!(crate::store::agent_tool_open::published(lineage, &open));
    assert!(validation::retained_request(&state, &open, 1).is_ok());
    assert!(validation::retained_request(&state, &open, 2).is_err());

    let runtime = state.resource.lineage.sandbox.runtime.as_mut().unwrap();
    runtime.provision_generation = Some(2);
    assert!(!crate::store::agent_tool_open::published(
        &state.resource.lineage,
        &open
    ));
}
