use super::*;
use futures_util::StreamExt;
use layer2_core::sandbox::store::{
    indexed::AgentOutputReader, AgentResponseLogStore, AgentResponseLogWindow,
};
use layer3_adapter::{ContentBlock, ContentChunk, MessageId};
use tokio::sync::{oneshot, Mutex};

struct CaptureBarrier {
    store: Arc<MongoSandboxStore>,
    indexed: bool,
    gate: Mutex<Option<(oneshot::Sender<Agent>, oneshot::Receiver<()>)>>,
}

impl CaptureBarrier {
    async fn arm(&self) -> (oneshot::Receiver<Agent>, oneshot::Sender<()>) {
        let (captured, wait) = oneshot::channel();
        let (resume, paused) = oneshot::channel();
        assert!(self.gate.lock().await.replace((captured, paused)).is_none());
        (wait, resume)
    }

    async fn pause(&self, agent: &Agent) {
        let gate = self.gate.lock().await.take();
        if let Some((captured, resume)) = gate {
            captured.send(agent.clone()).unwrap();
            resume.await.unwrap();
        }
    }
}

#[async_trait::async_trait]
impl AgentResponseLogStore for CaptureBarrier {
    async fn agent_output_reader(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<Box<dyn AgentOutputReader + '_>>, SandboxStoreError> {
        if self.indexed {
            self.pause(agent).await;
        }
        self.store.agent_output_reader(agent, turn).await
    }

    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        if !self.indexed {
            self.pause(agent).await;
        }
        self.store.agent_response_log_window(agent, turn).await
    }
}

fn request(
    agent: &Agent,
    method: axum::http::Method,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Request<Body> {
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("idempotency-key", "capture-advance-fixture")
        .body(payload.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(caller.subject.clone()));
    request
        .extensions_mut()
        .insert(caller.authorization.clone());
    request.extensions_mut().insert(CallerContext {
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: caller.subject.clone(),
            credential_id: "capture-advance-fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: caller.subject.clone(),
        },
        caller,
        memberships: vec![],
    });
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("capture-advance-fixture".into()));
    request
}

fn frame(bytes: &[u8]) -> Option<(u64, serde_json::Value)> {
    let text = std::str::from_utf8(bytes).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(text.lines().find_map(|line| line.strip_prefix("data: "))?).unwrap();
    assert_ne!(body["type"], "error", "{text}");
    let sequence = text
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap()
        .parse()
        .unwrap();
    Some((sequence, body))
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn original_post_recaptures_first_text_completion_and_cancellation() {
    for reason in [TurnStopReason::EndTurn, TurnStopReason::Cancelled] {
        tokio::time::timeout(Duration::from_secs(25), exercise(reason, false))
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn indexed_original_post_recaptures_normal_writer_commits() {
    tokio::time::timeout(
        Duration::from_secs(25),
        exercise(TurnStopReason::EndTurn, true),
    )
    .await
    .unwrap();
}

async fn exercise(reason: TurnStopReason, indexed: bool) {
    use axum::http::Method;
    use serde_json::json;
    let (client, name) = database("response_capture_advance").await;
    let (store, agent, _, execution, authority, _) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
        Some((100, true)),
    )
    .await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let writer_client = Client::with_uri_str(uri).await.unwrap();
    let reader = Arc::new((*store).clone().with_bounded_response_reads(true));
    let source = Arc::new(CaptureBarrier {
        store: reader.clone(),
        indexed,
        gate: Mutex::new(None),
    });
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let reads = AgentReadService::new(reader, Arc::new(NoResources))
        .with_resource_access(access)
        .with_response_logs(Some(source.clone()));
    let app = layer1_public::responses::v2::router(
        layer1_public::responses::v2::ResponsesV2State::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            commands,
            reads,
        )
        .with_paged_events(true),
    );
    let post = app
        .clone()
        .oneshot(request(
            &agent,
            Method::POST,
            "/api/v2/responses",
            Some(json!({"conversation":agent.id, "input":"question", "stream":true})),
        ))
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::OK);
    let response_id = post.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let writer = Arc::new(
        MongoSandboxStore::open(&writer_client, &name)
            .await
            .unwrap()
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap(),
    );
    let command_id = AgentCommandId(post.headers()["x-cma-command-id"].to_str().unwrap().into());
    AgentCommandWorker::new(store.clone(), execution, "capture-advance-worker".into())
        .unwrap()
        .run_one(&command_id)
        .await
        .unwrap();
    if indexed {
        let active = store.get_agent(&agent.id).await.unwrap().unwrap();
        super::super::runtime_loss_budget::adopt(
            &store,
            &active,
            AgentLogKind::ModeChanged {
                mode: "fixture-mode".into(),
            },
        )
        .await;
    }
    let recorder = DurableAgentUpdateRecorder::with_incremental_output_policy(
        writer.clone(),
        Arc::default(),
        None,
        false,
        false,
        None,
        true,
    );
    let mut assembly = AgentMessageAssembly::default();
    let (captured, resume) = source.arm().await;
    let mut body = post.into_body().into_data_stream();
    let reader = tokio::spawn(async move {
        let mut frames = Vec::new();
        loop {
            let bytes = body
                .next()
                .await
                .expect("POST closed before the first text")
                .unwrap();
            let Some(event) = frame(&bytes) else { continue };
            let is_text = event.1["type"] == "response.output_text.delta";
            frames.push(event);
            if is_text {
                return (body, frames);
            }
        }
    });
    let before = captured.await.unwrap();
    let turn = before.conversation.active_turn.unwrap();
    let generation = before.conversation.runtime_generation.unwrap();
    let retained = if indexed {
        None
    } else {
        Some(
            source
                .store
                .agent_response_log_window(&before, Some(turn))
                .await
                .unwrap()
                .expect("prepared window"),
        )
    };
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::from("committed answer"))
                    .message_id(MessageId::new("reply")),
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let after = writer.get_agent(&agent.id).await.unwrap().unwrap();
    let rows = writer
        .agent_log(&agent.id, before.conversation.last_seq, 32)
        .await
        .unwrap();
    assert_advance_identity(&before, &after, turn, &rows);
    let advanced = if indexed {
        source
            .store
            .agent_output_reader(&before, Some(turn))
            .await
            .err()
    } else {
        source
            .store
            .agent_response_log_window(&before, Some(turn))
            .await
            .err()
    };
    assert!(
        matches!(advanced, Some(SandboxStoreError::CaptureAdvanced)),
        "{advanced:?}"
    );
    resume.send(()).unwrap();
    let (mut body, mut frames) = reader.await.unwrap();
    let first_delta = frames.last().unwrap();
    assert_eq!(first_delta.1["delta"], "committed answer");
    let first_delta_id = first_delta.0;
    assert_eq!(
        writer
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .active_turn,
        Some(turn)
    );

    let terminal = if reason == TurnStopReason::Cancelled {
        "response.incomplete"
    } else {
        "response.completed"
    };
    let (captured, resume) = source.arm().await;
    // A quiet refresh reuses the first page without reopening the source.
    // Commit another chunk so the terminal race starts in a fresh capture.
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::from(" continued"))
                    .message_id(MessageId::new("reply")),
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let read = tokio::spawn(async move {
        while let Some(bytes) = body.next().await {
            frames.extend(frame(&bytes.unwrap()));
        }
        frames
    });
    let captured = captured.await.unwrap();
    assert_eq!(captured.conversation.active_turn, Some(turn));
    let native = before.conversation.conversation_id.as_deref().unwrap();
    let (acks, _received) = tokio::sync::mpsc::channel(1);
    let update = ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: uuid::Uuid::new_v4().to_string(),
                cursor: 1,
                native_conversation_id: native.into(),
                payload_sha256: "a".repeat(64),
            },
            acks,
        ),
        update: Some(
            Box::new(ConversationUpdate::TurnEndedForPrompt {
                prompt_id: PromptIdentity::new(native, generation, turn),
                response: PromptResponse::new(reason),
            })
            .into(),
        ),
    };
    recorder
        .record_update(
            &agent.id,
            generation,
            &mut assembly,
            update,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    if let Some(mut previous) = retained {
        assert!(matches!(
            source
                .store
                .agent_response_log_suffix(&captured, &previous)
                .await,
            Err(SandboxStoreError::CaptureAdvanced)
        ));
        previous.entries.last_mut().unwrap().appended_at = "changed boundary".into();
        assert!(matches!(
            source
                .store
                .agent_response_log_suffix(&captured, &previous)
                .await,
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
    resume.send(()).unwrap();
    let frames = read.await.unwrap();
    assert_eq!(frames.last().unwrap().1["type"], terminal);
    assert!(frames
        .iter()
        .enumerate()
        .all(|(index, (seq, _))| *seq == index as u64));
    let path = format!("/api/v2/responses/{response_id}/events");
    for header in [false, true] {
        let mut replay = if header {
            request(&agent, Method::GET, &path, None)
        } else {
            request(
                &agent,
                Method::GET,
                &format!("{path}?starting_after={first_delta_id}"),
                None,
            )
        };
        if header {
            replay
                .headers_mut()
                .insert("last-event-id", first_delta_id.to_string().parse().unwrap());
        }
        let reply = app.clone().oneshot(replay).await.unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let mut stream = reply.into_body().into_data_stream();
        let mut replay_frames = Vec::new();
        while let Some(bytes) = stream.next().await {
            replay_frames.extend(frame(&bytes.unwrap()));
        }
        assert_eq!(replay_frames, frames[first_delta_id as usize + 1..]);
    }
    assert_eq!(writer.agent_log(&agent.id, before.conversation.last_seq, 32).await.unwrap().iter().filter(|row| {
        matches!(&row.kind, AgentLogKind::AgentMessage { content } if content == "committed answer continued")
    }).count(), 1);
    client.database(&name).drop().await.unwrap();
}

fn assert_advance_identity(before: &Agent, after: &Agent, turn: u64, rows: &[AgentLogEntry]) {
    assert!(before.proves_output_advance(after, turn, rows));
    for change in [
        (|agent: &mut Agent| agent.owner = SubjectId::new("different-owner")) as fn(&mut Agent),
        |agent| agent.creation_key.push_str("-replaced"),
        |agent| agent.created_at.push_str("-replaced"),
        |agent| agent.conversation.runtime_generation = Some(99),
        |agent| agent.conversation.conversation_id = Some("different-conversation".into()),
        |agent| agent.conversation.active_turn = Some(99),
        |agent| agent.revision -= 1,
        |agent| agent.conversation.last_seq -= 1,
    ] {
        let mut mismatched = after.clone();
        change(&mut mismatched);
        assert!(!before.proves_output_advance(&mismatched, turn, rows));
    }
    assert!(!before.proves_output_advance(after, turn + 1, rows));
    assert!(!before.proves_output_advance(after, turn, &rows[1..]));
}
