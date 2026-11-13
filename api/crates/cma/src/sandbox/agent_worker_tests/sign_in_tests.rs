use super::*;
use layer2_core::sandbox::store::AgentWorkMutation;
use layer2_core::sandbox::AgentCleanupDebt;
use layer3_adapter::ConversationChannel;

pub(super) async fn opening_agent(harness: &Harness, name: &str) -> Agent {
    let admitted = harness.admit(name, vec![]).await;
    harness.run(&admitted.id, harness.now).await;
    harness.run(&admitted.id, harness.now).await;
    let mut agent = harness
        .store
        .claim_agent_work(
            &admitted.id,
            "agent-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.work.step, AgentWorkStep::OpenConversation);
    let expected = agent.revision;
    agent.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", agent.id),
        request_sha256: "a".repeat(64),
    });
    agent.revision += 1;
    harness
        .store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: expected,
            holder: "agent-worker".into(),
            fence: agent.work.lease.as_ref().unwrap().fence,
            agent: agent.clone(),
            worktree: None,
            appended_at: rfc3339_at(harness.now),
            sandbox_event: None,
            agent_logs: vec![],
        })
        .await
        .unwrap();
    let mut sandbox = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let expected = sandbox.revision;
    sandbox.profile.content.model_access = ModelAccess::OwnCredentials;
    sandbox.credential.sign_in = SandboxSignInState::Complete;
    sandbox.revision += 1;
    harness.store.save_sandbox(expected, sandbox).await.unwrap();
    agent
}

#[tokio::test]
async fn startup_sign_in_reaches_durable_reads_before_authentication_completes() {
    startup_sign_in_visible(false).await;
}

#[tokio::test]
async fn retained_sign_in_reaches_durable_reads_before_authentication_completes() {
    startup_sign_in_visible(true).await;
}

async fn startup_sign_in_visible(retained: bool) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut agent = opening_agent(&harness, "sign-in-durable").await;
    let initial = agent.clone();
    let parent = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let (sender, receiver) = mpsc::channel(4);
    let authentication = async {
        let notice = ConversationUpdate::SignInRequired {
            url: "https://login.example.test/device".into(),
            code: Some("fixture-code".into()),
            message: None,
        };
        let notice = if retained {
            ConversationUpdate::RetainedEffect(layer3_adapter::RuntimeOutputUpdate::new(
                notice, true,
            ))
        } else {
            notice
        };
        sender.send(notice).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let logs = harness
                    .store
                    .agent_log(&initial.id, initial.conversation.last_seq, 10)
                    .await
                    .unwrap();
                if logs
                    .iter()
                    .any(|entry| matches!(entry.kind, AgentLogKind::SignInRequired { .. }))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the user sees the challenge before authorizing the provider");
        let sandbox = harness
            .store
            .get_sandbox(&initial.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sandbox.credential.sign_in, SandboxSignInState::Required);
        sender
            .send(ConversationUpdate::Lost { frames: 2 })
            .await
            .unwrap();
        Ok(ConversationChannel {
            input: ConversationInputSender::spawn(4, |_| async { Ok(()) }),
            conversation_id: Some("authenticated-conversation".into()),
        })
    };
    let (channel, mut remaining) = harness
        .worker
        .await_startup_open(
            &mut agent,
            &parent,
            receiver,
            authentication,
            Duration::from_secs(2),
            harness.now,
        )
        .await
        .unwrap();
    assert_eq!(
        channel.conversation_id.as_deref(),
        Some("authenticated-conversation")
    );
    assert_eq!(agent.revision, initial.revision + 1);
    assert_eq!(agent.work, initial.work);
    assert_eq!(
        agent.conversation.last_seq,
        initial.conversation.last_seq + 1
    );
    assert!(matches!(
        remaining.recv().await,
        Some(ConversationUpdate::Lost { frames: 2 })
    ));
    assert!(agent.conversation.conversation_id.is_none());
}

#[tokio::test]
async fn startup_timeout_preserves_visible_challenge_and_cleanup_intent() {
    startup_timeout(false).await;
}

#[tokio::test]
async fn retained_startup_timeout_preserves_visible_challenge_and_cleanup_intent() {
    startup_timeout(true).await;
}

async fn startup_timeout(retained: bool) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut agent = opening_agent(&harness, "sign-in-timeout").await;
    let initial = agent.clone();
    let parent = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let (sender, receiver) = mpsc::channel(4);
    let opening = async {
        let notice = ConversationUpdate::SignInRequired {
            url: "https://login.example.test/device".into(),
            code: None,
            message: None,
        };
        let notice = if retained {
            ConversationUpdate::RetainedEffect(layer3_adapter::RuntimeOutputUpdate::new(
                notice, true,
            ))
        } else {
            notice
        };
        sender.send(notice).await.unwrap();
        std::future::pending::<layer3_adapter::Result<ConversationChannel>>().await
    };
    let error = harness
        .worker
        .await_startup_open(
            &mut agent,
            &parent,
            receiver,
            opening,
            Duration::from_millis(25),
            harness.now,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("agent_conversation_open_timeout"));
    let stored = harness.stored(&initial.id).await;
    assert_eq!(stored.work, initial.work);
    assert_eq!(
        stored.conversation.last_seq,
        initial.conversation.last_seq + 1
    );
    assert!(stored.conversation.conversation_id.is_none());
}
