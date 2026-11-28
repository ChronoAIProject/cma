use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, BodyEnd, CloseReason, Contract, Kind};

#[tokio::test(start_paused = true)]
async fn replayed_response_frames_are_quiet_until_new_durable_output_arrives() {
    isolated_test(async {
        let f = fixture().await;
        let response = create(&f, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0; 3]));
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(800), body.next())
                .await
                .is_err()
        );
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [3, 0, 0]));
        let record = f
            .store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .remove(0);
        f.store
            .reject_unclaimed_command(
                &record.receipt.envelope.command_id,
                record.revision,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let output = events(&body.next().await.unwrap().unwrap());
        assert_eq!(output[0]["type"], "response.failed");
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [3, 1, 0]));
        while body.next().await.is_some() {}
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::NormalTerminal
        );
        assert_eq!(snapshot(Kind::ResponsesV2), (0, [3, 1, 0]));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn expired_response_authorization_counts_failed_refresh_and_releases_stream() {
    crate::stream_telemetry::isolated_test(async {
        let f = fixture().await;
        let response = create(&f, false).await;
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0; 3]));
        drop(response);
        assert_eq!(snapshot(Kind::ResponsesV2), (0, [0; 3]));
        f.access
            .change_share(
                &context("owner-a"),
                Some("fixture-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: AgentId("agt_fixture".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "share".into(),
                },
            )
            .await
            .unwrap();
        let response = f
            .router
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/api/v2/responses/{id}/events"),
                "reader",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0; 3]));
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        let event = events(&body.next().await.unwrap().unwrap());
        assert_eq!(event[0]["type"], "error");
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0, 0, 1]));
        assert!(body.next().await.is_none());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::AuthorizationUnavailable
        );
        assert_eq!(
            f.store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items[0]
                .status,
            AgentResponseStatus::Queued
        );

        assert_eq!(
            crate::stream_telemetry::snapshot(crate::stream_telemetry::Kind::ResponsesV2),
            (0, [0, 0, 1])
        );
    })
    .await;
}

async fn semantic_stream(
    paged: bool,
    shared: bool,
    terminal: bool,
    refresh_barrier: Option<tokio::sync::watch::Receiver<bool>>,
) -> (Fixture, axum::response::Response) {
    let (base, store) = fixtures::conversation_fixture().await;
    fixtures::insert_agent(
        &store,
        fixtures::seeded_agent("agt_fixture", AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let reads = base.reads.with_resource_access(access.clone());
    let mut state = super::super::ResponsesV2State::new(
        crate::agent_page::AgentPageOrigin::test(),
        commands,
        reads,
    )
    .with_paged_events(paged);
    if let Some(start) = refresh_barrier {
        state = state.with_replay_refresh_barrier(0, start);
    }
    let f = Fixture {
        router: super::super::router(state.clone()),
        store,
        access,
    };
    drop(create(&f, false).await);
    if shared {
        f.access
            .change_share(
                &context("owner-a"),
                Some("fixture-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: AgentId("agt_fixture".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "semantic-share".into(),
                },
            )
            .await
            .unwrap();
    }
    let authorization = state
        .reads
        .authorize_event_stream(
            &context(if shared { "reader" } else { "owner-a" }),
            Some("fixture-credential"),
            &AgentId("agt_fixture".into()),
        )
        .await
        .unwrap();
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    if terminal {
        f.store
            .reject_unclaimed_command(
                &record.receipt.envelope.command_id,
                record.revision,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let receipt = state
        .commands
        .response_for_stream(&authorization, record.receipt.response_id.as_ref().unwrap())
        .await
        .unwrap()
        .command
        .receipt;
    let response = super::super::stream::open(state, authorization, receipt, None, None).await;
    (f, response)
}

#[tokio::test(start_paused = true)]
async fn replay_refresh_barrier_keeps_sse_heartbeats_running() {
    isolated_test(async {
        for paged in [false, true] {
            let (start, barrier) = tokio::sync::watch::channel(false);
            let (_f, response) = semantic_stream(paged, false, false, Some(barrier)).await;
            let mut body = response.into_body().into_data_stream();
            assert_eq!(
                events(&body.next().await.unwrap().unwrap())[0]["type"],
                "response.created"
            );
            let initial = snapshot(Kind::ResponsesV2).1;

            for _ in 0..2 {
                let heartbeat = tokio::time::timeout(Duration::from_secs(16), body.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert!(heartbeat.starts_with(b":"), "{heartbeat:?}");
                assert_eq!(snapshot(Kind::ResponsesV2).1, initial);
            }

            start.send_replace(true);
            assert!(tokio::time::timeout(Duration::from_secs(1), body.next())
                .await
                .is_err());
            assert!(snapshot(Kind::ResponsesV2).1[0] > initial[0]);
        }
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "replay fixture must explicitly release refreshes")]
async fn replay_refresh_barrier_requires_explicit_release() {
    let (start, barrier) = tokio::sync::watch::channel(false);
    let (_f, response) = semantic_stream(true, false, false, Some(barrier)).await;
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    drop(start);
    body.next().await;
}

#[tokio::test(start_paused = true)]
async fn legacy_and_paged_closes_preserve_failed_terminal_and_authorization_evidence() {
    for paged in [false, true] {
        let (_f, response) = semantic_stream(paged, false, true, None).await;
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        let terminal = body.next().await.unwrap().unwrap();
        assert_eq!(events(&terminal)[0]["type"], "response.failed");
        assert!(String::from_utf8_lossy(&terminal).contains("id: 1"));
        assert_eq!(
            evidence.close_reason(BodyEnd::Dropped),
            CloseReason::DropUnknown
        );
        assert!(body.next().await.is_none());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::NormalTerminal
        );

        let (f, response) = semantic_stream(paged, true, true, None).await;
        let command_id = layer2_core::ids::AgentCommandId(
            response.headers()["x-cma-command-id"]
                .to_str()
                .unwrap()
                .into(),
        );
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        let failure = body.next().await.unwrap().unwrap();
        assert_eq!(events(&failure)[0]["type"], "error");
        assert!(!String::from_utf8_lossy(&failure).contains("id: "));
        assert!(body.next().await.is_none());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::AuthorizationUnavailable
        );
        assert_eq!(
            f.store
                .get_command(&command_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentResponseStatus::Failed
        );
    }
}

#[tokio::test(start_paused = true)]
async fn legacy_and_paged_snapshot_failure_cannot_become_normal_eof() {
    for paged in [false, true] {
        let (f, response) = semantic_stream(paged, false, false, None).await;
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        bind_command(&f).await;
        let mut agent = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.revision += 1;
        agent.conversation.last_seq += 1;
        f.store.save_agent(revision, agent).await.unwrap();
        let frame = body.next().await.unwrap().unwrap();
        assert_eq!(
            events(&frame)[0]["code"],
            "response_observation_unavailable"
        );
        assert!(!String::from_utf8_lossy(&frame).contains("id: "));
        assert!(body.next().await.is_none());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::ServerError
        );
    }
}

struct AcceptingRuntime;
#[async_trait]
impl layer2_core::sandbox::AgentRuntimeRegistry for AcceptingRuntime {
    async fn resolve(
        &self,
        _: &layer2_core::sandbox::AgentRuntimeTarget,
    ) -> Result<layer2_core::sandbox::AgentRuntimeConnection, layer2_core::sandbox::AgentRuntimeError>
    {
        Ok(layer2_core::sandbox::AgentRuntimeConnection {
            capabilities: layer2_core::RuntimeCapabilities::new([
                layer2_core::RuntimeCapability::Conversation,
            ]),
            input: layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, |_| async { Ok(()) }),
        })
    }
    async fn fence(
        &self,
        _: &AgentId,
        _: u64,
    ) -> Result<(), layer2_core::sandbox::AgentRuntimeError> {
        Ok(())
    }
}
async fn bind_command(f: &Fixture) {
    let now = SystemTime::now();
    let claimed = f
        .store
        .claim_next_command(CommandClaimRequest {
            agent_id: AgentId("agt_fixture".into()),
            holder: "semantic-test".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    layer2_core::sandbox::AgentOperationService::new(f.store.clone(), Arc::new(AcceptingRuntime))
        .execute_claimed_command(&context("owner-a").caller, &claimed)
        .await
        .unwrap();
    f.store
        .release_command_claim(
            &claimed.work.lease.as_ref().unwrap().claim,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}
