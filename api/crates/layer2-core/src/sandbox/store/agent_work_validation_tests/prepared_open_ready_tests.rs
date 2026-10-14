use super::*;
use crate::sandbox::store::validate_prepared_agent_open_ready;

fn ready() -> (Agent, Worktree, AgentWorkMutation) {
    let (mut current, tree) = claimed(
        AgentWorkStep::OpenConversation,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current.origin = AgentOrigin::Lifecycle {
        operation_id: "prepared-operation".into(),
    };
    current.work.warm_conversation_id = Some("prepared-conversation".into());
    current.work.resolved_skills = Some(vec![]);
    current.profile.content.system_prompt.clear();
    current.work.receipts = vec![
        receipt(&current, AgentWorkStep::AddWorktree),
        receipt(&current, AgentWorkStep::MaterializeHome),
    ];
    current.work.cleanup_debt = vec![debt(&current, AgentWorkStep::OpenConversation)];
    let mut next = current.clone();
    next.conversation.conversation_id = current.work.warm_conversation_id.clone();
    next.conversation.runtime_generation = Some(1);
    next.work.cleanup_debt.clear();
    let mut opened = receipt(&current, AgentWorkStep::OpenConversation);
    opened.completed_at = AT.into();
    next.work.receipts.push(opened);
    let lease = current.work.lease.as_ref().unwrap();
    next.work
        .advance(
            &lease.holder,
            lease.fence,
            AgentWorkStep::PublishConnection,
            AT.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, AT.into(), None)
        .unwrap();
    next.conversation.last_seq += 3;
    let mut ready_tree = tree.clone();
    ready_tree.transition(WorktreeStatus::Ready).unwrap();
    let mutation = AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: lease.holder.clone(),
        fence: lease.fence,
        agent: next,
        worktree: Some(VersionedWorktree {
            expected_revision: tree.revision,
            worktree: ready_tree,
        }),
        appended_at: AT.into(),
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: current.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }),
        agent_logs: vec![
            completed(AgentWorkStep::OpenConversation),
            AgentLogKind::ConversationOpened {
                conversation_id: "prepared-conversation".into(),
                runtime_generation: 1,
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            },
        ],
    };
    (current, tree, mutation)
}

#[test]
fn prepared_open_ready_is_exact_and_requires_the_dedicated_validator() {
    let (current, tree, mutation) = ready();
    assert!(validate_prepared_agent_open_ready(
        &current,
        &tree,
        current.conversation.last_seq,
        &mutation
    )
    .is_ok());
    assert_conflict(&current, &tree, &mutation);
    for case in 0..16 {
        let (mut current, mut tree, mut mutation) = ready();
        match case {
            0 => mutation.agent.work.receipts[2].request_sha256 = "b".repeat(64),
            1 => mutation.agent.work.cleanup_debt = current.work.cleanup_debt.clone(),
            2 => mutation.agent.conversation.conversation_id = Some("foreign".into()),
            3 => mutation.agent.conversation.runtime_generation = Some(2),
            4 => {
                mutation.agent_logs.remove(0);
            }
            5 => mutation.sandbox_event = None,
            6 => mutation.worktree = None,
            7 => mutation.agent.revision += 1,
            8 => mutation.fence += 1,
            9 => current.work.lease.as_mut().unwrap().expires_at = AT.into(),
            10 => {
                current.origin = AgentOrigin::Manual;
                mutation.agent.origin = current.origin.clone();
            }
            11 => current.work.cleanup_debt.clear(),
            12 => tree.revision += 1,
            13 => mutation.agent.work.step = AgentWorkStep::None,
            14 => mutation
                .agent
                .work
                .receipts
                .push(receipt(&current, AgentWorkStep::CloseProcess)),
            _ => mutation.agent.ready_at = Some(NOW.into()),
        }
        assert!(
            validate_prepared_agent_open_ready(
                &current,
                &tree,
                current.conversation.last_seq,
                &mutation
            )
            .is_err(),
            "case {case}"
        );
    }
}
