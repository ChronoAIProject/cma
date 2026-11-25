use super::*;
#[path = "retained_output/measurement.rs"]
mod measurement;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_to;
use crate::store::sandbox_mongo_integration_tests::native_home_fixture::native_home;
use layer2_core::sandbox::{AgentConversationError, AgentOperationService, AgentService};
use measurement::{Counts, Meter};

struct NoWrites;
#[async_trait::async_trait]
impl layer2_core::OperationStore for NoWrites {
    async fn reserve(
        &self,
        _: &layer2_core::OperationClaim,
    ) -> Result<layer2_core::OperationReservation, layer2_core::StoreError> {
        panic!("retained read reserved input")
    }
    async fn complete(
        &self,
        _: &layer2_core::OperationClaim,
        _: Vec<u8>,
    ) -> Result<(), layer2_core::StoreError> {
        panic!("retained read acknowledged input")
    }
}

fn routes(store: Arc<MongoSandboxStore>, authority: Arc<Authority>) -> axum::Router {
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let reads =
        AgentReadService::new(store.clone(), Arc::new(NoResources)).with_resource_access(access);
    // No runtime is attached. All retained text is read from Mongo.
    let runtime = Arc::new(crate::sandbox::LiveAgentRegistry::default());
    let agents = AgentService::new(
        store.clone(),
        Arc::new(layer2_core::profiles::InMemoryProfileStore::default()),
        runtime.clone(),
        Default::default(),
    );
    let operations = AgentOperationService::new(store, runtime);
    let origin = layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap();
    layer1_public::rest::agent_router(layer1_public::rest::AgentRoutesState::new(
        origin.clone(),
        agents,
        reads.clone(),
        operations,
        Arc::new(NoWrites),
    ))
    .merge(layer1_public::responses::v2::router(
        layer1_public::responses::v2::ResponsesV2State::new(origin, commands, reads),
    ))
}

async fn get(router: &axum::Router, subject: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    let caller = Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(caller.subject.clone()));
    request
        .extensions_mut()
        .insert(caller.authorization.clone());
    request.extensions_mut().insert(CallerContext {
        credential_source: CredentialSource::Retained,
        caller: caller.clone(),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: caller.subject.clone(),
            credential_id: "retained-fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: caller.subject,
        },
        memberships: vec![],
    });
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("retained-fixture".into()));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    if status == StatusCode::OK && uri.contains("/retained-output") {
        assert_eq!(response.headers()["cache-control"], "private, no-store");
    }
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_output_maximum_prefix_survives_reload_old_reader_and_home_loss_without_receipt_changes(
) {
    let (_, name) = database("retained_output_public").await;
    let meter = Arc::new(Meter::default());
    let client = meter.client().await;
    let mut parent = active_sandbox("sbx_commands");
    parent.lifecycle.home = Some(Box::new(native_home(&parent.id)));
    let binding = parent.runtime.as_mut().unwrap();
    binding.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    binding.boot_id = Some("b".repeat(32));
    parent.validate().unwrap();
    let (store, agent, parent, execution, authority, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        parent,
        Some((100, true)),
    )
    .await;
    let command = record(
        store
            .reserve_command(live_request(&agent, &parent, "retained-public"))
            .await
            .unwrap(),
    );
    let id = &command.receipt.envelope.command_id;
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "retained-public-worker".into()).unwrap();
    worker.run_one(id).await.unwrap();
    // Publish the command's execution observation while normal log capacity is
    // available. The later text-only checkpoint must not need a new log row.
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from("")),
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let full = fill_to(&store, current, None, Some(64 * 1024 * 1024)).await;
    let store = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap(),
    );
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let text = format!("{}x", "界".repeat((1024 * 1024 - 1) / 3));
    let mut assembly = AgentMessageAssembly::default();
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let source = |cursor| RuntimeOutputSource {
        stream_id: "93111111-1111-4111-8111-111111111111".into(),
        cursor,
        native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
        payload_sha256: "d".repeat(64),
    };
    crate::sandbox::record_update_for_test(
        recorder.as_ref(),
        &agent.id,
        1,
        &mut assembly,
        ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(source(1), acks.clone()),
            update: Some(
                Box::new(ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                    layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                        text.clone(),
                    )),
                )))
                .into(),
            ),
        },
    )
    .await
    .unwrap();
    assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
    assert_eq!(
        crate::sandbox::record_update_for_test(
            recorder.as_ref(),
            &agent.id,
            1,
            &mut assembly,
            ConversationUpdate::RuntimeOutput {
                receipt: RuntimeOutputReceipt::new(source(2), acks),
                update: Some(
                    Box::new(ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(
                        ToolCallUpdate::new(
                            "refused",
                            ToolCallUpdateFields::new().title("not acknowledged")
                        )
                    )))
                    .into()
                ),
            }
        )
        .await,
        Err(AgentConversationError::CapacityExceeded)
    );
    assert!(acknowledged.try_recv().is_err());
    worker.run_one(id).await.unwrap();
    let settled = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(settled.status, AgentResponseStatus::Failed);
    assert_eq!(settled.receipt, command.receipt);
    let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let prefix = row["agent_capacity_prefix_v1"].clone();
    let checkpoint = store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.as_ref().unwrap().cursor, 1);
    // The existing reader ignores the additive top-level sidecar, exactly as
    // binaries before the public feature do. Ordinary saves preserve it.
    let mut without = row.clone();
    without.remove("agent_capacity_prefix_v1");
    assert_eq!(
        crate::store::agents::read_agent(&row).unwrap(),
        crate::store::agents::read_agent(&without).unwrap()
    );
    let mut revised = saved.clone();
    revised.revision += 1;
    store.save_agent(saved.revision, revised).await.unwrap();
    // A native home/runtime loss cannot remove Mongo's retained sidecar. No
    // resource reader or runtime is available to the public retrieval routes.
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    assert!(parent
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .is_some());
    parent.runtime = None;
    parent.lifecycle.home = None;
    parent.revision += 1;
    store.save_sandbox(revision, parent).await.unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let original_terminal = reopened
        .agent_log(&agent.id, full.conversation.last_seq, 64)
        .await
        .unwrap();
    // Layer 2 derives typed discovery from this server-owned historical notice;
    // it must identify the same atomic settlement that actually stored a prefix.
    assert!(original_terminal.iter().any(|entry| matches!(
        &entry.kind,
        AgentLogKind::TurnFailed { failure, .. }
            if failure.class == "agent_log_capacity_exceeded"
                && failure.detail == "This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export."
    )));
    let original_agent = reopened.get_agent(&agent.id).await.unwrap();
    let router = routes(reopened.clone(), authority);
    let route = format!("/api/v1/agents/{}/retained-output", agent.id);
    let response_route = format!(
        "/api/v2/responses/{}/retained-output",
        settled.receipt.response_id.as_ref().unwrap()
    );
    let ordinary = format!(
        "/api/v2/responses/{}",
        settled.receipt.response_id.as_ref().unwrap()
    );
    let before = get(&router, agent.owner.as_str(), &ordinary).await;
    assert_eq!(before.0, StatusCode::OK);
    assert_eq!(before.1["status"], "failed");
    assert_eq!(
        before.1["retained_output"],
        serde_json::json!({"available":true})
    );
    let mut cursor = None;
    let mut actual = String::new();
    let mut chunks = 0;
    let mut totals = [Counts::default(), Counts::default()];
    loop {
        let query = cursor
            .as_ref()
            .map_or(String::new(), |value| format!("?cursor={value}"));
        meter.start();
        let (status, page) = get(&router, agent.owner.as_str(), &format!("{route}{query}")).await;
        totals[0].add(meter.finish());
        assert_eq!(status, StatusCode::OK, "{page}");
        meter.start();
        let response_page = get(
            &router,
            agent.owner.as_str(),
            &format!("{response_route}{query}"),
        )
        .await;
        totals[1].add(meter.finish());
        assert_eq!((status, page.clone()), response_page);
        assert_eq!(
            page["output"]["turn_sequence"].as_u64(),
            full.conversation.active_turn
        );
        assert_eq!(
            page["output"]["checkpoint_sequence"],
            full.conversation.last_seq
        );
        assert_eq!(page["output"]["partial"], true);
        assert_eq!(page["output"]["byte_length"], text.len());
        assert_eq!(page["offset"], actual.len());
        let content = page["content"].as_str().unwrap();
        assert!(content.len() <= 512 * 1024);
        actual.push_str(content);
        chunks += 1;
        cursor = page["next"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(actual, text);
    assert_eq!(chunks, 3);
    for ((surface, commands, reply_bytes), total) in [
        ("agent", 72, 24 * 1024 * 1024),
        ("response", 96, 30 * 1024 * 1024),
    ]
    .into_iter()
    .zip(totals)
    {
        assert_eq!(total.checkpoint_reads, chunks);
        assert_eq!(total.failed, 0);
        assert!(
            total.commands <= commands,
            "{surface}: {} commands",
            total.commands
        );
        assert!(
            total.reply_bytes <= reply_bytes,
            "{surface}: {} BSON bytes",
            total.reply_bytes
        );
        println!("retained-output surface={surface} text_bytes={} chunks={chunks} commands={} reply_bson_bytes={}", text.len(), total.commands, total.reply_bytes);
    }
    assert_eq!(
        get(&router, "foreign", &route).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&router, "foreign", &response_route).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &router,
            agent.owner.as_str(),
            &format!("{route}?cursor=foreign")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(
            &router,
            agent.owner.as_str(),
            "/api/v1/agents/unknown/retained-output"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&router, agent.owner.as_str(), &ordinary).await, before);
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), original_agent);
    assert_eq!(
        reopened
            .agent_log(&agent.id, full.conversation.last_seq, 64)
            .await
            .unwrap(),
        original_terminal
    );
    assert_eq!(reopened.get_command(id).await.unwrap().unwrap(), settled);
    assert_eq!(
        reopened
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap(),
        checkpoint
    );
    assert_eq!(
        reopened
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()["agent_capacity_prefix_v1"],
        prefix
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert!(acknowledged.try_recv().is_err());
    // Missing original failure evidence fails closed, without returning text.
    reopened
        .agent_log
        .delete_one(
            doc! {"agent_id": agent.id.as_str(), "seq": (full.conversation.last_seq + 1) as i64},
        )
        .await
        .unwrap();
    let (status, body) = get(&router, agent.owner.as_str(), &route).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(!body.to_string().contains('界'));
    client.database(&name).drop().await.unwrap();
}
