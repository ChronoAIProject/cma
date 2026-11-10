use super::*;

#[path = "reads/fences.rs"]
mod fences;
#[path = "reads/history.rs"]
mod history_tests;
#[path = "reads/stream.rs"]
mod stream;
#[path = "reads/window.rs"]
mod window;
#[path = "reads/window_probe_tests.rs"]
mod window_probe_tests;

async fn history(store: &InMemorySandboxStore) -> Agent {
    let agent = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    replace_history(
        store,
        agent,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "private-native-id".into(),
                runtime_generation: 7,
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: Some("private-origin".into()),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 3,
                metadata: [("private".into(), "private-metadata".into())].into(),
            },
            AgentLogKind::OwnerMessage {
                content: "Please review".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Review complete".into(),
            },
            AgentLogKind::SignInRequired {
                url: "https://private-sign-in.test".into(),
                code: Some("private-code".into()),
                message: None,
            },
            AgentLogKind::Usage {
                input_tokens: 5,
                output_tokens: 8,
                context_used: 13,
                context_window: None,
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: "runtime_failed".into(),
                    detail: "private-host and private-path".into(),
                },
            },
        ],
        Some("private-native-id"),
        Some(7),
        None,
    )
    .await
}

#[tokio::test]
async fn shared_detail_history_and_usage_preserve_records_and_personal_projections() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access.clone()));
    let original = reads
        .lineage(&context("owner").caller, &agent.id)
        .await
        .unwrap();
    let original_log = reads
        .log(&context("owner").caller, &agent.id, 0, 200)
        .await
        .unwrap();
    let original_transcript = reads
        .transcript(&context("owner").caller, &agent.id)
        .await
        .unwrap();
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let personal = reads
        .detail_authorized(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let AgentDetailReading::Personal(personal) = personal else {
        panic!("personal projection")
    };
    assert_eq!(*personal, original);
    assert_eq!(
        reads
            .transcript_authorized(&context("owner"), None, &agent.id)
            .await
            .unwrap(),
        original_transcript
    );
    let AgentDetailReading::Shared(shared) = reads
        .detail_authorized(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap()
    else {
        panic!("shared projection")
    };
    assert_eq!(shared.id, agent.id);
    assert_eq!(shared.model.as_deref(), Some("model-a"));
    let transcript = reads
        .transcript_authorized(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let encoded = serde_json::to_string(&transcript).unwrap();
    assert!(!encoded.contains("private-"), "{encoded}");
    assert!(encoded.contains("Review complete"));
    assert!(encoded.contains("runtime_failed"));
    let usage = reads
        .usage_authorized(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    assert_eq!(usage.usage.input_tokens, 5);
    assert_eq!(usage.usage.output_tokens, 8);
    let mut after = 0;
    let mut visible = Vec::new();
    loop {
        let page = reads
            .log_authorized(
                &context("recipient"),
                Some("request-credential"),
                &agent.id,
                after,
                2,
            )
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        after = page.last().unwrap().seq;
        visible.extend(page);
    }
    assert_eq!(visible.len(), original_log.len());
    for (shared, stored) in visible.iter().zip(&original_log) {
        assert_eq!(
            (shared.seq, &shared.appended_at),
            (stored.seq, &stored.appended_at)
        );
        assert!(!serde_json::to_string(&shared.kind)
            .unwrap()
            .contains("private-"));
    }
    assert!(visible[0].kind.is_none());
    assert!(visible[1].kind.is_none());
    assert!(visible[3].kind.is_none());
    assert!(visible[6].kind.is_none());
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        store.agent_log(&agent.id, 0, 200).await.unwrap(),
        original_log
    );
}

#[tokio::test]
async fn narrow_history_never_grants_siblings_legacy_lineage_or_sandbox_metrics() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    insert_hierarchy(
        &store,
        "sibling",
        "sibling-tree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let resources = Arc::new(StubResources::reporting(SandboxResourceReading::default()));
    let reads = AgentReadService::new(store.clone(), resources.clone())
        .with_resource_access(Arc::new(access.clone()));
    let recipient = context("recipient");
    let id = AgentId("agent".into());
    assert!(matches!(
        reads.lineage(&recipient.caller, &id).await,
        Err(AgentReadError::NotFound)
    ));
    assert!(matches!(
        reads
            .detail_authorized(
                &recipient,
                Some("request-credential"),
                &AgentId("sibling".into())
            )
            .await,
        Err(AgentReadError::NotFound)
    ));
    assert!(matches!(
        reads
            .resources_authorized(&recipient, Some("request-credential"), &id)
            .await,
        Err(AgentReadError::Forbidden)
    ));
    assert!(resources.seen.lock().unwrap().is_empty());
    let sandbox = ResourceSelector::Sandbox {
        sandbox_id: SandboxId("sandbox".into()),
    };
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&sandbox, "whole-sandbox"),
        )
        .await
        .unwrap();
    let reading = reads
        .resources_authorized(&recipient, Some("request-credential"), &id)
        .await
        .unwrap();
    assert_eq!(reading.sandbox_id.as_str(), "sandbox");
    assert_eq!(resources.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn removal_and_outage_block_all_shared_reads_without_affecting_owner() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    let agent = history(&store).await;
    let receipt = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let reads = service(store).with_resource_access(Arc::new(access.clone()));
    for reply in [Reply::Missing, Reply::Outage] {
        *authority.reply.lock().unwrap() = reply;
        let recipient = context("recipient");
        assert!(reads
            .detail_authorized(&recipient, Some("request-credential"), &agent.id)
            .await
            .is_err());
        assert!(reads
            .transcript_authorized(&recipient, Some("request-credential"), &agent.id)
            .await
            .is_err());
        assert!(reads
            .log_authorized(&recipient, Some("request-credential"), &agent.id, 0, 2)
            .await
            .is_err());
        assert!(reads
            .usage_authorized(&recipient, Some("request-credential"), &agent.id)
            .await
            .is_err());
        assert!(reads
            .resources_authorized(&recipient, Some("request-credential"), &agent.id)
            .await
            .is_err());
        assert!(reads
            .transcript_authorized(&context("owner"), None, &agent.id)
            .await
            .is_ok());
    }
    *authority.reply.lock().unwrap() = Reply::Role(OrganizationRole::Viewer);
    assert!(reads
        .transcript_authorized(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .is_ok());
    access
        .change_share(&context("owner"), None, revoke(&selector, &receipt.share))
        .await
        .unwrap();
    assert!(matches!(
        reads
            .transcript_authorized(&context("recipient"), Some("request-credential"), &agent.id)
            .await,
        Err(AgentReadError::NotFound)
    ));
}

#[tokio::test]
async fn corrupt_shared_history_cannot_be_hidden_by_event_redaction() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let mut agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let previous = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq += 1; // Exact snapshot promises a record absent from the retained log.
    store.save_agent(previous, agent.clone()).await.unwrap();
    let reads = service(store).with_resource_access(Arc::new(access));
    let recipient = context("recipient");
    assert!(matches!(
        reads
            .log_authorized(&recipient, Some("request-credential"), &agent.id, 0, 2)
            .await,
        Err(AgentReadError::Corrupt(_))
    ));
    assert!(matches!(
        reads
            .transcript_authorized(&recipient, Some("request-credential"), &agent.id)
            .await,
        Err(AgentReadError::Corrupt(_))
    ));
    assert!(matches!(
        reads
            .usage_authorized(&recipient, Some("request-credential"), &agent.id)
            .await,
        Err(AgentReadError::Corrupt(_))
    ));
}
