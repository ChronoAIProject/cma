use super::*;

#[path = "provider_failure_http.rs"]
mod provider_failure;

struct BindingStore {
    inner: Arc<InMemorySandboxStore>,
    bind_after_response_reads: usize,
    bound: AtomicBool,
    response_reads: AtomicUsize,
}

impl BindingStore {
    fn record(&self, mut record: AdmittedAgentCommand) -> AdmittedAgentCommand {
        if !self.bound.load(Ordering::SeqCst) {
            return record;
        }
        record.status = AgentResponseStatus::InProgress;
        record.work.dispatch = Some(CommandDispatch {
            claim: CommandClaim {
                agent_id: record.receipt.envelope.agent_id.clone(),
                command_id: record.receipt.envelope.command_id.clone(),
                holder: "indexed-test".into(),
                fence: 1,
            },
            operation_sequence: 1,
            turn_sequence: 3,
            runtime_generation: 1,
            fenced_at: AT.into(),
            delivery: layer2_core::sandbox::AgentOperationDelivery::DispatchFenced,
        });
        record
    }
}

#[async_trait]
impl AgentCommandStore for BindingStore {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.inner.reserve_command(request).await
    }

    async fn get_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let record = self.inner.get_command(id).await?;
        Ok(record.map(|record| self.record(record)))
    }

    async fn get_response_command(
        &self,
        id: &layer2_core::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let record = self.inner.get_response_command(id).await?;
        if record.is_some()
            && self.response_reads.fetch_add(1, Ordering::SeqCst) + 1
                >= self.bind_after_response_reads
        {
            self.bound.store(true, Ordering::SeqCst);
        }
        Ok(record.map(|record| self.record(record)))
    }
}

#[tokio::test]
async fn legacy_http_pages_oversized_output_without_truncating_the_response() {
    let mut kinds = beginning();
    kinds.extend((0..12).map(|_| message(&"a".repeat(400_000))));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let router = crate::responses::router(f.state.clone());
    let path = "/api/v1/responses/resp_agt_indexed_3";
    let (status, body) = crate::rest::agents_tests::json_response(
        &router,
        request(Method::GET, path, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "response_pagination_required");
    let mut from = 0;
    let mut count = 0;
    loop {
        let (status, body) = crate::rest::agents_tests::json_response(
            &router,
            request(
                Method::GET,
                &format!("{path}?output_page=true&output_after={from}"),
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(serde_json::to_vec(&body).unwrap().len() <= 4 * 1024 * 1024);
        count += body["data"].as_array().unwrap().len();
        if body["has_more"] == false {
            break;
        }
        let next = body["next_after"].as_u64().unwrap();
        assert!(next > from);
        from = next;
    }
    assert_eq!(count, 12);
    let (status, _) = crate::rest::agents_tests::json_response(
        &router,
        request(
            Method::GET,
            &format!("{path}?output_page=true&output_after=99"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert!(!status.is_success());
}

#[tokio::test]
async fn legacy_indexed_stream_retains_intermediate_tools_and_fences_buffered_frames() {
    let mut kinds = beginning();
    kinds.extend([
        tool("call", "running"),
        output("call", "first"),
        update("call", "completed"),
        output("call", "latest"),
        message("answer"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    let response = crate::responses::stream::open_authorized(
        f.state.clone(),
        crate::AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        },
        f.agent.clone(),
        3,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut all = vec![];
    while let Some(frame) = stream.next().await {
        all.extend(events(&frame.unwrap()));
    }
    assert_eq!(all.last().unwrap()["type"], "response.completed");
    let activity = all
        .iter()
        .filter(|e| e["type"] == "response.cma_chat.tool_activity")
        .collect::<Vec<_>>();
    assert_eq!(activity.len(), 4);
    assert_eq!(activity[1]["item"]["output"]["text"], "first");
    assert_eq!(activity[3]["item"]["output"]["text"], "latest");
    let response = crate::responses::stream::open_authorized(
        f.state.clone(),
        crate::AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        },
        f.agent.clone(),
        3,
        None,
    )
    .await;
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(
        events(&stream.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    f.purge().await;
    assert!(
        stream.next().await.is_none(),
        "buffered frames cannot escape deleted authority"
    );
}

#[tokio::test]
async fn unbound_submit_switches_to_indexed_output_after_binding() {
    let data = make({
        let mut kinds = beginning();
        kinds.extend([
            message("answer"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        kinds
    })
    .await;
    let command_store = Arc::new(BindingStore {
        inner: data.store.clone(),
        bind_after_response_reads: 2,
        bound: AtomicBool::new(false),
        response_reads: AtomicUsize::new(0),
    });
    let commands = layer2_core::sandbox::agent_io::AgentCommandService::new(
        command_store,
        data.access.clone(),
        Arc::new(Authority),
    );
    let state = crate::responses::v2::ResponsesV2State::new(
        data.state.agent_page_origin.clone(),
        commands,
        data.reads.clone(),
    );
    let router = crate::responses::v2::router(state);
    let response = router
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some("indexed-transition"),
            Some(json!({
                "conversation": "agt_indexed",
                "input": "Hello",
                "stream": true
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(data.source.counts.opens.load(Ordering::SeqCst), 0);
    let mut body = response.into_body().into_data_stream();
    let first = events(&body.next().await.unwrap().unwrap());
    assert_eq!(first[0]["type"], "response.created");
    assert!(first.iter().all(|event| event["type"] != "error"));
    assert_eq!(data.source.counts.opens.load(Ordering::SeqCst), 0);
    let mut completed = false;
    for _ in 0..8 {
        let frame = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let frame = events(&frame);
        assert!(frame.iter().all(|event| event["type"] != "error"));
        if frame
            .iter()
            .any(|event| event["type"] == "response.completed")
        {
            completed = true;
            break;
        }
    }
    assert!(completed, "indexed continuation never completed");
    assert!(data.source.counts.opens.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn unbound_submit_can_bind_before_first_snapshot_without_source_probe() {
    let data = make({
        let mut kinds = beginning();
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        kinds
    })
    .await;
    let command_store = Arc::new(BindingStore {
        inner: data.store.clone(),
        bind_after_response_reads: 1,
        bound: AtomicBool::new(false),
        response_reads: AtomicUsize::new(0),
    });
    let commands = layer2_core::sandbox::agent_io::AgentCommandService::new(
        command_store,
        data.access.clone(),
        Arc::new(Authority),
    );
    let state = crate::responses::v2::ResponsesV2State::new(
        data.state.agent_page_origin.clone(),
        commands,
        data.reads.clone(),
    );
    let router = crate::responses::v2::router(state);
    let response = router
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some("indexed-first-snapshot"),
            Some(json!({
                "conversation": "agt_indexed",
                "input": "Hello",
                "stream": true
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(data.source.counts.opens.load(Ordering::SeqCst), 0);
    let bytes = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    let events = events(&bytes);
    assert!(events.iter().all(|event| event["type"] != "error"));
    assert!(events
        .iter()
        .any(|event| event["type"] == "response.created"));
    assert!(events
        .iter()
        .any(|event| event["type"] == "response.completed"));
    assert!(data.source.counts.opens.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn replay_rebuilds_indexed_output_when_custody_expires_during_a_page() {
    check_indexed_handoff("answer".into(), None).await;
}

#[tokio::test]
async fn replay_pagination_error_releases_recovery_before_eof() {
    check_indexed_handoff(
        "\\".repeat(OUTPUT_PAGE_BYTES / 2 + 1),
        Some("response_pagination_required"),
    )
    .await;
}

#[tokio::test]
async fn replay_projection_failure_during_custody_loss_does_not_read_again() {
    check_indexed_handoff("answer".into(), Some("response_observation_unavailable")).await;
}

async fn check_indexed_handoff(answer: String, expected_error: Option<&'static str>) {
    use layer2_core::sandbox::agent_observation::observation_scope;

    let data = make({
        let mut kinds = beginning();
        kinds.push(message(&answer));
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        kinds
    })
    .await;
    let command_store = Arc::new(BindingStore {
        inner: data.store.clone(),
        bind_after_response_reads: 1,
        bound: AtomicBool::new(false),
        response_reads: AtomicUsize::new(0),
    });
    let commands = layer2_core::sandbox::agent_io::AgentCommandService::new(
        command_store.clone(),
        data.access.clone(),
        Arc::new(Authority),
    );
    let (reads, custody, _) = super::super::shared_tests::install_with_commands(
        data.reads.clone(),
        data.store.clone(),
        command_store,
        &data.agent,
    );
    let state = crate::responses::v2::ResponsesV2State::new(
        data.state.agent_page_origin.clone(),
        commands,
        reads,
    );
    let slots = state.durable_recovery_slots.clone();
    let capacity = slots.available_permits();
    let router = crate::responses::v2::router(state);
    let submit = observation_scope(
        custody.clone(),
        router.clone().oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some("indexed-handoff"),
            Some(json!({
                "conversation": "agt_indexed",
                "input": "Hello",
                "stream": true
            })),
        )),
    )
    .await
    .unwrap();
    assert_eq!(submit.status(), StatusCode::OK);
    let id = submit.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(submit);
    let response = observation_scope(
        custody.clone(),
        router.oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events?starting_after=0"),
            "owner-a",
            None,
            None,
        )),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-cma-response-id"], id);

    let fail_projection = expected_error == Some("response_observation_unavailable");
    data.source.counts.stall.store(true, Ordering::SeqCst);
    data.source
        .counts
        .fail_points
        .store(fail_projection, Ordering::SeqCst);
    let collected = tokio::spawn(async move {
        let mut body = response.into_body().into_data_stream();
        let mut released = Vec::new();
        while let Some(frame) = body.next().await {
            let frame = frame.unwrap();
            let events = events(&frame);
            if events.iter().any(|event| event["type"] == "error") {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0]["code"].as_str(), expected_error);
                assert!(!frame
                    .windows(b"private output fixture".len())
                    .any(|part| part == b"private output fixture"));
                assert!(!std::str::from_utf8(&frame)
                    .unwrap()
                    .lines()
                    .any(|line| line.starts_with("id:")));
                assert_eq!(slots.available_permits(), capacity);
                assert!(body.next().await.is_none());
                return;
            }
            released.extend(events);
        }
        assert!(expected_error.is_none(), "expected a connection error");
        assert!(released.iter().all(|event| event["type"] != "error"));
        for (index, event) in released.iter().enumerate() {
            assert_eq!(event["sequence_number"], (index + 1) as u64);
        }
        let completed = released.last().unwrap();
        assert_eq!(completed["type"], "response.completed");
        assert_eq!(completed["response"]["id"], id);
    });
    tokio::time::timeout(
        Duration::from_secs(4),
        data.source.counts.entered.notified(),
    )
    .await
    .expect("indexed page entered before closure");
    let opens = data.source.counts.opens.load(Ordering::SeqCst);
    custody.revoke();
    data.source.counts.stall.store(false, Ordering::SeqCst);
    data.source.counts.release.notify_one();
    tokio::time::timeout(Duration::from_secs(4), collected)
        .await
        .unwrap()
        .unwrap();
    if fail_projection {
        assert_eq!(data.source.counts.opens.load(Ordering::SeqCst), opens);
    }
}

#[tokio::test]
async fn v2_output_page_http_preserves_authority_and_refuses_a_foreign_cursor() {
    let mut kinds = beginning();
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let data = make(kinds).await;
    let f = fixture_with_events(Some(data.source.clone()), false).await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let path = format!("/api/v2/responses/{id}/output");
    let (status, page) = read_json(&f.router, Method::GET, &path, "owner-a", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["data"], json!([]));
    assert_eq!(page["has_more"], false);
    let (status, _) = read_json(
        &f.router,
        Method::GET,
        &format!("{path}?after=legacy:0"),
        "owner-a",
        None,
    )
    .await;
    assert!(!status.is_success());
    let (status, _) = read_json(&f.router, Method::GET, &path, "unrelated", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
