use super::*;

#[test]
fn startup_notice_preserves_claim_identity_and_accepts_only_bounded_sign_in_evidence() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::OpenConversation,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current
        .work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::OpenConversation));
    let mutation = publication(
        &current,
        current.clone(),
        vec![AgentLogKind::SignInRequired {
            url: "https://login.example.test/device".into(),
            code: Some("fixture-code".into()),
            message: Some("Authenticate your account".into()),
        }],
        None,
        None,
    );
    assert_valid(&current, &worktree, &mutation);
    for variant in 0..9 {
        let mut wrong = mutation.clone();
        match variant {
            0 => wrong.fence += 1,
            1 => wrong.agent.work.cleanup_debt.clear(),
            2 => wrong.agent.work.lease = None,
            3 => wrong.agent.conversation.conversation_id = Some("invented".into()),
            4 => {
                wrong.agent_logs = vec![AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::OpenConversation,
                }]
            }
            5 => {
                wrong.agent_logs = vec![AgentLogKind::SignInRequired {
                    url: "a".repeat(4097),
                    code: None,
                    message: None,
                }]
            }
            6 => {
                wrong.agent_logs = vec![AgentLogKind::SignInRequired {
                    url: String::new(),
                    code: None,
                    message: None,
                }]
            }
            7 => {
                wrong.agent_logs = vec![AgentLogKind::SignInRequired {
                    url: "https://login.example.test/".into(),
                    code: Some("bad\0value".into()),
                    message: None,
                }]
            }
            _ => {
                wrong.appended_at = LEASE_END.into();
                wrong.agent.updated_at = LEASE_END.into();
            }
        }
        assert_conflict(&current, &worktree, &wrong);
    }
    let mut without_intent = current.clone();
    without_intent.work.cleanup_debt.clear();
    let mut forged = publication(
        &without_intent,
        without_intent.clone(),
        mutation.agent_logs.clone(),
        None,
        None,
    );
    assert_conflict(&without_intent, &worktree, &forged);
    forged.agent.work.resolved_skills = Some(vec![]);
    assert_conflict(&without_intent, &worktree, &forged);

    let mut already_opened = current.clone();
    already_opened.conversation.conversation_id = Some("existing-conversation".into());
    already_opened.conversation.runtime_generation = Some(1);
    let repeated = publication(
        &already_opened,
        already_opened.clone(),
        mutation.agent_logs,
        None,
        None,
    );
    assert_conflict(&already_opened, &worktree, &repeated);
}
