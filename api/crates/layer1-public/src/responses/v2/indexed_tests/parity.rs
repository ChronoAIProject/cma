use super::*;

#[tokio::test]
async fn committed_text_is_visible_while_open_and_every_resume_keeps_its_event_ids() {
    let delta = |offset, content: &str| AgentLogKind::AgentMessageDelta {
        offset,
        content: content.into(),
    };
    let mut kinds = beginning();
    kinds.push(delta(0, "é"));
    let open = make(kinds.clone()).await;
    let prefix = suffix(&open, 0).await;
    assert_eq!(prefix, projection::events(&open.golden(), true));
    let last = prefix.last().unwrap();
    assert_eq!(last["type"], "response.output_text.delta");
    assert_eq!(last["delta"], "é");
    let cursor = last["sequence_number"].as_u64().unwrap() + 1;
    kinds.extend([
        delta(2, "clair"),
        message("éclair"),
        tool("call", "completed"),
        delta(0, "done"),
        message("done"),
    ]);
    for terminal in [
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::TurnInterrupted { turn_sequence: 3 },
    ] {
        let mut closed_kinds = kinds.clone();
        closed_kinds.push(terminal);
        let closed = make(closed_kinds).await;
        let expected = projection::events(&closed.golden(), true);
        assert_eq!(&expected[..cursor as usize], prefix.as_slice());
        assert_eq!(expected[cursor as usize]["delta"], "clair");
        assert_eq!(expected[cursor as usize]["item_id"], last["item_id"]);
        for next in cursor..=expected.len() as u64 {
            assert_eq!(suffix(&closed, next).await, expected[next as usize..]);
        }
    }
}

pub(super) async fn suffix(f: &IndexedFixture, next: u64) -> Vec<Value> {
    let reading = f.reading.clone();
    f.observe(move |source| {
        Box::pin(async move {
            let mut next = next;
            let mut result = vec![];
            loop {
                let page = stream_page::read(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading.clone(),
                    next,
                )
                .await?;
                assert!(!page.pagination_required);
                if let Some(last) = page.events.last() {
                    next = last["sequence_number"].as_u64().unwrap() + 1;
                }
                result.extend(page.events);
                if page.terminal || !page.more {
                    break;
                }
            }
            Ok(result)
        })
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn every_replay_suffix_matches_original_messages_tools_and_terminal_output() {
    let mut kinds = beginning();
    kinds.extend([
        message(""),
        tool("first", "running"),
        output("first", "at completion"),
        message("interleaved"),
        update("first", "completed"),
        output("first", "latest"),
        tool("first", "failed"),
        tool("instant", "failed"),
        message("final"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::Usage {
            input_tokens: 7,
            output_tokens: 11,
            context_used: 18,
            context_window: None,
        },
    ]);
    let f = make(kinds).await;
    let expected = f.golden();
    let events = projection::events(&expected, true);
    let reading = f.reading.clone();
    let actual = f
        .observe(move |source| {
            Box::pin(async move {
                projection::indexed::complete(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                )
                .await
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert!(serde_json::to_string(&actual).unwrap().contains("latest"));
    for next in 0..=events.len() {
        assert_eq!(
            suffix(&f, next as u64).await,
            events[next..],
            "suffix {next}"
        );
    }
    let id = f.agent.id.clone();
    let reading = f.reading.clone();
    let error = f
        .observe(move |source| {
            Box::pin(async move {
                assert_eq!(source.metadata().agent_id, id);
                stream_page::read(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                    100_000,
                )
                .await
                .map(|_| ())
            })
        })
        .await
        .unwrap_err();
    assert!(matches!(error, AgentReadError::InvalidPage));
}

#[tokio::test]
async fn failure_gap_and_post_terminal_usage_match_the_original_projection() {
    for terminal in [
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::TurnInterrupted { turn_sequence: 3 },
        AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: layer2_core::sandbox::TypedFailure {
                class: "insufficient_credits".into(),
                detail: "terminal fixture".into(),
            },
        },
    ] {
        let mut kinds = beginning();
        kinds.extend([
            message("answer"),
            AgentLogKind::ConversationGap { frames: 2 },
            terminal,
            AgentLogKind::UsageUpdated {
                input_tokens: 13,
                output_tokens: 17,
                turn_sequence: Some(3),
                turn_input_tokens: 13,
                turn_output_tokens: 17,
                context_used: 30,
                context_window: None,
            },
        ]);
        let f = make(kinds).await;
        assert_eq!(suffix(&f, 0).await, projection::events(&f.golden(), true));
    }
}

#[tokio::test]
async fn exact_deferred_failure_binding_stays_queued() {
    let mut kinds = beginning();
    kinds.extend([
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 2,
            operation: layer2_core::sandbox::AgentOperationKind::SendMessage,
            state: layer2_core::sandbox::AgentOperationDelivery::PreWriteFailed,
        },
        AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: layer2_core::sandbox::TypedFailure {
                class: SOURCE_DEFERRED_FAILURE.into(),
                detail: "private deferred marker".into(),
            },
        },
    ]);
    let mut f = make(kinds).await;
    f.reading.deferred_failure_detail = Some("private deferred marker".into());
    let expected = f.golden();
    assert_eq!(
        expected.status,
        crate::responses::v2::V2ResponseStatus::Queued
    );
    assert_eq!(suffix(&f, 0).await, projection::events(&expected, true));
}

#[tokio::test]
async fn legacy_retrieval_uses_original_message_ordinals_and_current_usage() {
    let mut kinds = beginning();
    kinds.extend([
        tool("first", "completed"),
        message("one"),
        message("two"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::Usage {
            input_tokens: 2,
            output_tokens: 3,
            context_used: 5,
            context_window: None,
        },
    ]);
    let f = make(kinds).await;
    let rows = f.source.data.rows.values().cloned().collect::<Vec<_>>();
    let expected = crate::responses::retrieve::project(
        &crate::agent_page::AgentPageOrigin::test(),
        &f.agent,
        &rows,
        3,
    )
    .unwrap();
    let agent = f.agent.clone();
    let actual = f
        .observe(move |source| {
            Box::pin(async move {
                crate::responses::retrieve::indexed::complete(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    &agent,
                    3,
                )
                .await
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[tokio::test]
async fn retained_discovery_survives_indexed_reads_and_every_replay_suffix() {
    for (detail, available) in [
        ("This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export.", true),
        ("Durable Agent history reached its lifetime budget. Retained output and runtime identity are preserved. This Agent cannot continue under the current limits; inspect capacity before explicit recovery or replacement.", false),
    ] {
        let mut kinds = beginning();
        kinds.push(AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: layer2_core::sandbox::TypedFailure {
                class: "agent_log_capacity_exceeded".into(),
                detail: detail.into(),
            },
        });
        let f = make(kinds).await;
        let expected = f.golden();
        assert_eq!(expected.retained_output.is_some(), available);
        let reading = f.reading.clone();
        let indexed = f.observe(move |source| Box::pin(async move {
            projection::indexed::complete(source, &crate::agent_page::AgentPageOrigin::test(), reading).await
        })).await.unwrap().unwrap();
        assert_eq!(serde_json::to_value(indexed).unwrap(), serde_json::to_value(&expected).unwrap());
        let events = projection::events(&expected, true);
        assert!(events[..2].iter().all(|frame| frame["response"].get("retained_output").is_none()));
        for next in 0..=events.len() {
            assert_eq!(suffix(&f, next as u64).await, events[next..]);
        }
    }
}
