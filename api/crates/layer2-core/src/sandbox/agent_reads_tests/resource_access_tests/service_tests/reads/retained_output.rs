use super::*;
use crate::sandbox::store::{AgentMessageCheckpoint, AGENT_LOG_CAPACITY_FAILURE};
use crate::sandbox::{AgentMessageAssembly, AgentUpdateOutcome, RETAINED_OUTPUT_CHUNK_BYTES};

pub(super) async fn retained(
    store: &InMemorySandboxStore,
    content: String,
) -> (Agent, AgentMessageCheckpoint) {
    let agent = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let agent = replace_history(
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
            AgentLogKind::OwnerMessage {
                content: "Explain".into(),
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: AGENT_LOG_CAPACITY_FAILURE.into(),
                    detail: "private-capacity-detail".into(),
                },
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Working,
                to: AgentStatus::Failed,
            },
        ],
        Some("private-native-id"),
        Some(7),
        None,
    )
    .await;
    let prefix = AgentMessageCheckpoint {
        version: 1,
        update_id: "e9111111-1111-4111-8111-111111111111".into(),
        update_fingerprint: "a".repeat(64),
        native_conversation_id: "private-native-id".into(),
        runtime_generation: 7,
        active_turn: Some(3),
        after_seq: 4,
        assembly: AgentMessageAssembly {
            content,
            ..Default::default()
        },
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
        output: None,
    };
    store.retain_agent_output_for_test(&agent.id, prefix.clone());
    (agent, prefix)
}

#[tokio::test]
async fn retained_output_maximum_multibyte_chunks_are_exact_and_read_only() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let content = format!("{}x", "界".repeat((1024 * 1024 - 1) / 3));
    assert_eq!(content.len(), 1024 * 1024);
    let (agent, prefix) = retained(&store, content.clone()).await;
    let original = store.agent_log(&agent.id, 0, 200).await.unwrap();
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let shared = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let mut cursor = None;
    let mut text = String::new();
    let mut chunks = 0;
    loop {
        let page = reads
            .retained_output(&context("owner").caller, &agent.id, cursor.as_deref())
            .await
            .unwrap();
        assert_eq!(
            page,
            reads
                .retained_output_for_stream(&shared, Some(3), cursor.as_deref())
                .await
                .unwrap()
        );
        assert_eq!(page.offset, text.len());
        assert!(page.content.len() <= RETAINED_OUTPUT_CHUNK_BYTES);
        let output = page.output.as_ref().unwrap();
        assert_eq!(
            (
                output.turn_sequence,
                output.checkpoint_sequence,
                output.failure_sequence
            ),
            (3, 4, 5)
        );
        assert_eq!(output.byte_length, content.len());
        assert!(!serde_json::to_string(&page).unwrap().contains("private-"));
        text.push_str(&page.content);
        chunks += 1;
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(chunks, 3);
    assert_eq!(text, content);
    assert!(reads
        .retained_output_for_stream(&shared, Some(99), None)
        .await
        .unwrap()
        .output
        .is_none());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(store.agent_log(&agent.id, 0, 200).await.unwrap(), original);
    assert_eq!(
        crate::sandbox::store::AgentReadQueryStore::retained_agent_output(store.as_ref(), &agent)
            .await
            .unwrap(),
        Some(prefix)
    );
}

#[tokio::test]
async fn retained_output_refuses_foreign_unknown_revoked_and_expired_authority() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let (agent, _) = retained(&store, "visible".repeat(100_000)).await;
    let share = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap()
        .share;
    let reads = service(store).with_resource_access(Arc::new(access.clone()));
    assert!(matches!(
        reads
            .retained_output(&context("foreign").caller, &agent.id, None)
            .await,
        Err(AgentReadError::NotFound)
    ));
    assert!(matches!(
        reads
            .retained_output(&context("owner").caller, &AgentId("unknown".into()), None)
            .await,
        Err(AgentReadError::NotFound)
    ));
    let held = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let page = reads
        .retained_output_for_stream(&held, None, None)
        .await
        .unwrap();
    assert!(page.next.is_some(), "revocation must fence a continuation");
    access
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    assert!(reads
        .retained_output_for_stream(&held, None, page.next.as_deref())
        .await
        .is_err());
    *authority.reply.lock().unwrap() = Reply::Outage;
    assert!(reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .is_err());
    assert!(reads
        .retained_output(&context("owner").caller, &agent.id, None)
        .await
        .is_ok());
}

#[tokio::test]
async fn retained_output_absence_bad_cursors_and_corrupt_evidence_are_not_partial_successes() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let owner = context("owner").caller;
    let id = AgentId("agent".into());
    assert!(reads
        .retained_output(&owner, &id, None)
        .await
        .unwrap()
        .output
        .is_none());
    let (agent, prefix) = retained(&store, "界".repeat(30_000)).await;
    let page = reads.retained_output(&owner, &id, None).await.unwrap();
    let identity = &page.output.unwrap().id;
    for cursor in [
        "garbage".into(),
        format!("{identity}:1"),
        format!("{identity}:9999999"),
        "foreign:0".into(),
    ] {
        assert!(matches!(
            reads.retained_output(&owner, &id, Some(&cursor)).await,
            Err(AgentReadError::InvalidPage)
        ));
    }
    for mode in ["version", "turn", "head", "closed", "oversize", "foreign"] {
        let mut bad = prefix.clone();
        match mode {
            "version" => bad.version = 99,
            "turn" => bad.active_turn = Some(2),
            "head" => bad.after_seq = 5,
            "closed" => bad.outcome.turn_closed = true,
            "oversize" => bad.assembly.content = "x".repeat(1024 * 1024 + 1),
            "foreign" => {
                bad.outcome.appended = vec![AgentLogEntry {
                    agent_id: AgentId("foreign".into()),
                    seq: 4,
                    appended_at: NOW.into(),
                    kind: AgentLogKind::OwnerMessage {
                        content: "foreign".into(),
                    },
                }]
            }
            _ => unreachable!(),
        }
        store.retain_agent_output_for_test(&id, bad);
        assert!(
            matches!(
                reads.retained_output(&owner, &id, None).await,
                Err(AgentReadError::Corrupt(_))
            ),
            "{mode}"
        );
    }
    store.retain_agent_output_for_test(&id, prefix);
    let original = store.agent_log(&id, 0, 200).await.unwrap();
    for mode in ["gap", "completed", "wrong_turn", "other_failure", "status"] {
        let mut entries = original.clone();
        match mode {
            "gap" => {
                entries.remove(4);
            }
            "completed" => entries[4].kind = AgentLogKind::TurnCompleted { turn_sequence: 3 },
            "wrong_turn" => {
                entries[4].kind = AgentLogKind::TurnFailed {
                    turn_sequence: 99,
                    failure: TypedFailure {
                        class: AGENT_LOG_CAPACITY_FAILURE.into(),
                        detail: "failure".into(),
                    },
                }
            }
            "other_failure" => {
                entries[4].kind = AgentLogKind::TurnFailed {
                    turn_sequence: 3,
                    failure: TypedFailure {
                        class: "other".into(),
                        detail: "failure".into(),
                    },
                }
            }
            "status" => {
                entries[5].kind = AgentLogKind::StatusChanged {
                    from: AgentStatus::Working,
                    to: AgentStatus::Ready,
                }
            }
            _ => unreachable!(),
        }
        store.replace_agent_log_for_test(&agent.id, entries);
        assert!(
            matches!(
                reads.retained_output(&owner, &id, None).await,
                Err(AgentReadError::Corrupt(_))
            ),
            "{mode}"
        );
    }
}

#[tokio::test]
async fn retained_output_binds_historical_failure_after_generation_recovery() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let (agent, _) = retained(&store, "original partial text".into()).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let before = reads
        .retained_output(&context("owner").caller, &agent.id, None)
        .await
        .unwrap();
    let mut recovered = agent.clone();
    recovered.revision += 1;
    recovered.conversation.runtime_generation = Some(8);
    recovered.conversation.last_seq += 1;
    store.save_agent(agent.revision, recovered).await.unwrap();
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::ConversationOpened {
                conversation_id: "private-native-id".into(),
                runtime_generation: 8,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        reads
            .retained_output(&context("owner").caller, &agent.id, None)
            .await
            .unwrap(),
        before
    );
}
