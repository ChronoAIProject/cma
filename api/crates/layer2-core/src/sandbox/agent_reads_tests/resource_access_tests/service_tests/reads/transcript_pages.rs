use super::*;
use crate::sandbox::AgentTranscriptPageRequest;

fn first(limit: u16) -> AgentTranscriptPageRequest {
    AgentTranscriptPageRequest {
        turn: None,
        after: None,
        through: None,
        limit,
    }
}

#[tokio::test]
async fn legacy_transcript_pages_preserve_exact_serialization_and_shared_redaction() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    for subject in ["owner", "recipient"] {
        let expected = reads
            .transcript_authorized(&context(subject), Some("request-credential"), &agent.id)
            .await
            .unwrap();
        for limit in [1, 2, 32] {
            let mut request = first(limit);
            let mut result = AgentTranscript {
                agent_id: agent.id.clone(),
                through_seq: agent.conversation.last_seq,
                turns: vec![],
            };
            loop {
                let page = reads
                    .transcript_page_authorized(
                        &context(subject),
                        Some("request-credential"),
                        &agent.id,
                        &request,
                    )
                    .await
                    .unwrap();
                for mut turn in page.transcript.turns {
                    if let Some(last) = result
                        .turns
                        .last_mut()
                        .filter(|last| last.turn_sequence == turn.turn_sequence)
                    {
                        last.messages.append(&mut turn.messages);
                        last.agent_messages.append(&mut turn.agent_messages);
                    } else {
                        result.turns.push(turn);
                    }
                }
                match page.next {
                    Some(next) => request = next,
                    None => break,
                }
            }
            assert_eq!(
                serde_json::to_vec(&result).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
    }
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn transcript_pages_preserve_owner_capability_denial_and_reject_stale_continuations() {
    let (store, _, _, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let reads = service(store.clone());
    let mut caller = context("owner").caller;
    caller.authorization = crate::Authorization::none();
    assert!(matches!(
        reads
            .transcript_page(&caller, &AgentId("missing".into()), &first(4))
            .await,
        Err(AgentReadError::Forbidden)
    ));
    let mut stale = first(4);
    stale.turn = Some(3);
    stale.after = Some(4);
    stale.through = Some(agent.conversation.last_seq - 1);
    assert!(reads
        .transcript_page(&context("owner").caller, &agent.id, &stale)
        .await
        .is_err());
}

#[tokio::test]
async fn certified_usage_serializes_identically_to_the_legacy_fold_across_generations_and_overrides(
) {
    let (store, _, _, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let base = history(&store).await;
    let original = store.agent_log(&base.id, 0, 200).await.unwrap();
    for observed in [false, true] {
        let mut agent = base.clone();
        let mut entries = original.clone();
        let mut kinds = vec![
            AgentLogKind::Usage {
                input_tokens: u64::MAX,
                output_tokens: 20,
                context_used: 7,
                context_window: Some(100),
            },
            AgentLogKind::Usage {
                input_tokens: 1,
                output_tokens: 2,
                context_used: 0,
                context_window: None,
            },
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation: 7,
                runtime_generation: 8,
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "private-native-id".into(),
                runtime_generation: 9,
            },
            AgentLogKind::Usage {
                input_tokens: 3,
                output_tokens: 4,
                context_used: 5,
                context_window: Some(200),
            },
        ];
        if observed {
            kinds.push(AgentLogKind::UsageUpdated {
                input_tokens: 100,
                output_tokens: 200,
                context_used: 12,
                context_window: Some(512),
                turn_sequence: Some(3),
                turn_input_tokens: 10,
                turn_output_tokens: 20,
            });
        }
        for kind in kinds {
            entries.push(AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: entries.len() as u64 + 1,
                appended_at: NOW.into(),
                kind,
            });
        }
        agent.conversation.last_seq = entries.len() as u64;
        agent.conversation.runtime_generation = Some(9);
        let legacy = crate::sandbox::agent_reads_fold::fold_agent_log(&agent, &entries)
            .unwrap()
            .usage;
        let mut state = crate::sandbox::AgentLogProjectionState::default();
        for entry in &entries {
            state
                .observe(
                    &agent,
                    entry,
                    crate::sandbox::AgentLogProjectionState::queued_owner(entry),
                )
                .unwrap();
        }
        state.validate_aggregate(&agent).unwrap();
        let projected = crate::sandbox::AgentUsageReading {
            agent_id: agent.id,
            through_seq: agent.conversation.last_seq,
            usage: state.usage(),
        };
        assert_eq!(
            serde_json::to_vec(&projected).unwrap(),
            serde_json::to_vec(&legacy).unwrap()
        );
    }
}
