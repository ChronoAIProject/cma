use super::*;

#[tokio::test]
async fn responses_post_streams_committed_text_and_resumes_after_the_original_delta() {
    use layer3_adapter::{
        ContentBlock, ContentChunk, ConversationUpdate, MessageId, PromptIdentity, PromptResponse,
        SessionUpdate, TurnStopReason,
    };

    for (reason, terminal_type) in [
        (TurnStopReason::EndTurn, "response.completed"),
        (TurnStopReason::Cancelled, "response.incomplete"),
    ] {
        let f = super::fixture().await;
        let response = f
            .router
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                "owner",
                Some(json!({"conversation":"agt_composed", "input":"question", "stream":true})),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response_id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        bind(&f).await;
        let id = AgentId("agt_composed".into());
        let current = f.store.get_agent(&id).await.unwrap().unwrap();
        let turn = current.conversation.active_turn.unwrap();
        let generation = current.conversation.runtime_generation.unwrap();
        let recorder = DurableAgentUpdateRecorder::with_incremental_output_policy(
            f.store.clone(),
            Arc::default(),
            None,
            false,
            false,
            None,
            true,
        );
        let chunk = |text: &str| {
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::from(text))
                    .message_id(MessageId::new("fixture-message")),
            ))
        };
        let mut assembly = AgentMessageAssembly::default();
        recorder
            .record_update(
                &id,
                generation,
                &mut assembly,
                chunk("first "),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        align_response_turn(&f, &id, turn).await;
        let mut post = response.into_body().into_data_stream();
        let mut wire = String::new();
        let first_id = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let bytes = post
                    .next()
                    .await
                    .expect("POST closed before first delta")
                    .unwrap();
                wire.push_str(std::str::from_utf8(&bytes).unwrap());
                if let Some((id, delta)) = sse_text_deltas(&wire).into_iter().next() {
                    assert_eq!(delta["delta"], "first ");
                    return id;
                }
            }
        })
        .await
        .expect("first committed delta while the turn was open");
        assert_eq!(
            f.store
                .get_agent(&id)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .active_turn,
            Some(turn)
        );
        drop(post);

        let replacement = DurableAgentUpdateRecorder::new(f.store.clone());
        let mut assembly = AgentMessageAssembly::default();
        replacement
            .record_update(
                &id,
                generation,
                &mut assembly,
                chunk("second"),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        replacement
            .record_update(
                &id,
                generation,
                &mut assembly,
                ConversationUpdate::TurnEndedForPrompt {
                    prompt_id: PromptIdentity::new(
                        current.conversation.conversation_id.as_deref().unwrap(),
                        generation,
                        turn,
                    ),
                    response: PromptResponse::new(reason),
                },
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        align_response_turn(&f, &id, turn).await;
        let path = format!("/api/v2/responses/{response_id}/events");
        for by_header in [false, true] {
            let mut replay = if by_header {
                request(Method::GET, &path, "owner", None)
            } else {
                request(
                    Method::GET,
                    &format!("{path}?starting_after={first_id}"),
                    "owner",
                    None,
                )
            };
            if by_header {
                replay
                    .headers_mut()
                    .insert("last-event-id", first_id.to_string().parse().unwrap());
            }
            let response = f.router.clone().oneshot(replay).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = tokio::time::timeout(
                Duration::from_secs(10),
                axum::body::to_bytes(response.into_body(), 64 * 1024),
            )
            .await
            .unwrap()
            .unwrap();
            let wire = std::str::from_utf8(&bytes).unwrap();
            let deltas = sse_text_deltas(wire);
            assert_eq!(deltas.len(), 1);
            assert_eq!(deltas[0].0, first_id + 1);
            assert_eq!(deltas[0].1["delta"], "second");
            assert!(wire.contains(terminal_type));
        }
        let final_messages = f.source.state.lock().unwrap().rows.iter().filter(|entry| {
            entry.seq > turn && matches!(&entry.kind, AgentLogKind::AgentMessage { content } if content == "first second")
        }).count();
        assert_eq!(final_messages, 1);
    }
}

async fn align_response_turn(f: &Fixture, id: &AgentId, turn: u64) {
    let agent = f.store.get_agent(id).await.unwrap().unwrap();
    let rows = f
        .store
        .agent_log(id, 0, agent.conversation.last_seq.try_into().unwrap())
        .await
        .unwrap();
    f.source.align_turn(&agent, turn, rows);
}

fn sse_text_deltas(wire: &str) -> Vec<(u64, Value)> {
    wire.split("\n\n")
        .filter_map(|frame| {
            let id = frame
                .lines()
                .find_map(|line| line.strip_prefix("id: "))?
                .parse()
                .ok()?;
            let body: Value =
                serde_json::from_str(frame.lines().find_map(|line| line.strip_prefix("data: "))?)
                    .ok()?;
            (body["type"] == "response.output_text.delta").then_some((id, body))
        })
        .collect()
}
