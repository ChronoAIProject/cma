use super::*;
use crate::hosted::HostedControlTransport;

#[tokio::test]
async fn native_http_bounds_idle_sessions_without_exhausting_request_stream_permits() {
    let fixture = fixture().await;
    let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
        unreachable!()
    };
    let mut handles = Vec::new();
    for index in 0..40 {
        let name = format!("claimed-{index}");
        let handle = RuntimeHandle::new(format!("agent-sandbox:{name}:{}", sandbox_uid(&name)));
        fixture
            .state
            .runtimes
            .lock()
            .unwrap()
            .insert(name.clone(), client.control_token(&name));
        fixture
            .transport
            .bind_native_runtime(&handle, &format!("sandbox-{index}"), &"a".repeat(64))
            .await
            .unwrap();
        fixture
            .transport
            .exchange(&handle, RequestFrame::Resources)
            .await
            .unwrap();
        assert!(
            fixture.transport.state.lock().await.native_http.len()
                <= native_pool::MAX_HTTP_SESSIONS
        );
        handles.push(handle);
    }
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 40);
    for handle in handles {
        fixture.transport.retire(&handle).await.unwrap();
    }
    fixture.drained().await;
}

#[tokio::test]
async fn native_http_evicts_the_least_recently_used_idle_connection_not_the_first_name() {
    let fixture = fixture().await;
    let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
        unreachable!()
    };
    let mut handles = Vec::new();
    for index in 0..=native_pool::MAX_HTTP_SESSIONS {
        let name = format!("claimed-{index:02}");
        let handle = RuntimeHandle::new(format!("agent-sandbox:{name}:{}", sandbox_uid(&name)));
        fixture
            .state
            .runtimes
            .lock()
            .unwrap()
            .insert(name.clone(), client.control_token(&name));
        fixture
            .transport
            .bind_native_runtime(&handle, &format!("sandbox-{index}"), &"a".repeat(64))
            .await
            .unwrap();
        if index == native_pool::MAX_HTTP_SESSIONS {
            fixture
                .transport
                .exchange(&handles[0], RequestFrame::Resources)
                .await
                .unwrap();
        }
        fixture
            .transport
            .exchange(&handle, RequestFrame::Resources)
            .await
            .unwrap();
        handles.push(handle);
    }
    let opened = native_pool::MAX_HTTP_SESSIONS + 1;
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), opened);
    fixture
        .transport
        .exchange(&handles[0], RequestFrame::Resources)
        .await
        .unwrap();
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), opened);
    fixture
        .transport
        .exchange(&handles[1], RequestFrame::Resources)
        .await
        .unwrap();
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), opened + 1);
    for handle in handles {
        fixture.transport.retire(&handle).await.unwrap();
    }
    fixture.drained().await;
}

async fn fixture() -> Fixture {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"a".repeat(64))
        .await
        .unwrap();
    fixture
}

async fn resources(fixture: &Fixture) -> crate::Result<ResponseFrame> {
    fixture
        .transport
        .exchange(&handle(), RequestFrame::Resources)
        .await
}

async fn received(fixture: &Fixture, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.requests.lock().unwrap().len() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request reached the original stream");
}

fn start_resources(fixture: &Fixture) -> tokio::task::JoinHandle<crate::Result<ResponseFrame>> {
    let transport = fixture.transport.clone();
    tokio::spawn(async move { transport.exchange(&handle(), RequestFrame::Resources).await })
}

#[tokio::test]
async fn native_http_reuses_one_exec_for_status_mutation_and_history_with_fresh_pod_reads() {
    let fixture = fixture().await;
    let status = RequestFrame::RuntimeStatus(crate::RuntimeStatusRequest {
        sandbox_id: "sandbox-one".into(),
        generation: 1,
        home_identity: "a".repeat(43),
    });
    fixture.transport.exchange(&handle(), status).await.unwrap();
    assert_eq!(fixture.state.pod_reads.load(Ordering::SeqCst), 3);
    let install = RequestFrame::Install(crate::InstallRequest::new("install-once", vec![]));
    assert!(matches!(
        fixture
            .transport
            .exchange(&handle(), install)
            .await
            .unwrap(),
        ResponseFrame::Install(_)
    ));
    assert_eq!(fixture.state.pod_reads.load(Ordering::SeqCst), 4);
    assert_eq!(
        fixture
            .transport
            .export_agent_history(&handle(), &AgentSlot::new("agent-a"))
            .await
            .unwrap(),
        b"{\"history\":[]}"
    );
    assert_eq!(fixture.state.pod_reads.load(Ordering::SeqCst), 5);
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
    let endpoint = fixture.endpoint().await;
    let mut socket = fixture
        .transport
        .connect(&endpoint, &AgentSlot::new("agent-a"))
        .await
        .unwrap();
    socket.send(Message::Binary(vec![1, 2, 3])).await.unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(vec![1, 2, 3])
    );
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 2);
    drop(socket);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_http_refuses_replaced_pod_and_restarted_container_before_reused_request_bytes() {
    for restart in [false, true] {
        let fixture = fixture().await;
        resources(&fixture).await.unwrap();
        if restart {
            fixture.state.restarted.store(true, Ordering::SeqCst);
        } else {
            fixture.state.replaced.store(true, Ordering::SeqCst);
        }
        assert!(matches!(
            resources(&fixture).await,
            Err(AdapterError::Rejected(_))
        ));
        fixture.drained().await;
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        assert!(fixture.transport.state.lock().await.native_http.is_empty());
    }
}

#[tokio::test]
async fn native_http_checks_current_pod_after_waiting_for_exclusive_checkout() {
    let fixture = fixture().await;
    resources(&fixture).await.unwrap();
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let first = start_resources(&fixture);
    received(&fixture, 2).await;
    let before = fixture.state.pod_reads.load(Ordering::SeqCst);
    let second = start_resources(&fixture);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        fixture.state.pod_reads.load(Ordering::SeqCst),
        before,
        "queued call cannot spend its proof before checkout"
    );
    fixture.state.restarted.store(true, Ordering::SeqCst);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    first.await.unwrap().unwrap();
    assert!(matches!(
        second.await.unwrap(),
        Err(AdapterError::Rejected(_))
    ));
    fixture.drained().await;
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_http_concurrent_calls_serialize_without_mixing_status_and_mutation_replies() {
    let fixture = fixture().await;
    resources(&fixture).await.unwrap();
    let install = RequestFrame::Install(crate::InstallRequest::new("concurrent-install", vec![]));
    let runtime = handle();
    let (a, b) = tokio::join!(
        resources(&fixture),
        fixture.transport.exchange(&runtime, install)
    );
    assert!(matches!(a.unwrap(), ResponseFrame::Resources(_)));
    assert!(matches!(b.unwrap(), ResponseFrame::Install(_)));
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_http_lost_partial_error_oversized_and_invalid_replies_poison_without_replay() {
    for fault in 1..=6 {
        let fixture = fixture().await;
        resources(&fixture).await.unwrap();
        fixture.state.fault.store(fault, Ordering::SeqCst);
        let request = if fault == 1 {
            RequestFrame::Install(crate::InstallRequest::new("single-effect", vec![]))
        } else {
            RequestFrame::Resources
        };
        assert!(
            fixture
                .transport
                .exchange(&handle(), request)
                .await
                .is_err(),
            "fault {fault}"
        );
        fixture.drained().await;
        assert_eq!(
            fixture.state.requests.lock().unwrap().len(),
            2,
            "fault {fault} must not resend"
        );
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        resources(&fixture).await.unwrap();
        assert_eq!(
            fixture.state.opened.load(Ordering::SeqCst),
            2,
            "fault {fault} cannot reuse sender"
        );
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn native_http_cancellation_poisons_checked_out_connection() {
    let fixture = fixture().await;
    resources(&fixture).await.unwrap();
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let active = start_resources(&fixture);
    received(&fixture, 2).await;
    active.abort();
    let _ = active.await;
    fixture.drained().await;
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    resources(&fixture).await.unwrap();
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 2);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_http_retirement_cancels_active_and_queued_requests_and_rejects_old_endpoints() {
    let fixture = fixture().await;
    resources(&fixture).await.unwrap();
    let stale = fixture.endpoint().await;
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let first = start_resources(&fixture);
    received(&fixture, 2).await;
    let second = start_resources(&fixture);
    tokio::time::sleep(Duration::from_millis(20)).await;
    fixture.transport.retire(&handle()).await.unwrap();
    assert!(first.await.unwrap().is_err());
    assert!(second.await.unwrap().is_err());
    fixture.drained().await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"a".repeat(64))
        .await
        .unwrap();
    assert!(fixture
        .transport
        .control(&stale, &RequestFrame::Resources)
        .await
        .is_err());
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    assert!(fixture.transport.state.lock().await.native_http.is_empty());
}

#[tokio::test]
async fn native_http_retirement_cancels_connection_opening_before_any_runtime_bytes() {
    let fixture = Fixture::start(
        Mode::StallBeforeReady,
        NativeRuntimeTransport::KubernetesExec,
    )
    .await;
    let operation = start_resources(&fixture);
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.transport.retire(&handle()).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(1), operation)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    fixture.drained().await;
    assert!(fixture.state.requests.lock().unwrap().is_empty());
    assert!(fixture.transport.state.lock().await.native_http.is_empty());
}

#[tokio::test]
async fn native_http_full_request_identity_change_invalidates_idle_stream() {
    let fixture = fixture().await;
    resources(&fixture).await.unwrap();
    let stale = fixture.endpoint().await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"b".repeat(64))
        .await
        .unwrap();
    fixture.drained().await;
    assert!(fixture
        .transport
        .control(&stale, &RequestFrame::Resources)
        .await
        .is_err());
    resources(&fixture).await.unwrap();
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 2);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_http_idle_expiry_releases_exec_without_another_request() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let body = serde_json::to_vec(&RequestFrame::Resources).unwrap();
    let reply = fixture
        .transport
        .native_http(
            &endpoint,
            http::Method::POST,
            endpoint.control().unwrap(),
            body,
            8192,
        )
        .await
        .unwrap();
    assert!(reply.status.is_success());
    reply
        .lease
        .recycle_with_idle_ttl(Duration::from_millis(30))
        .await
        .unwrap();
    fixture.drained().await;
    assert!(fixture.transport.state.lock().await.native_http.is_empty());
}

#[tokio::test]
async fn native_http_never_reuses_a_pod_without_complete_container_incarnation() {
    let fixture = fixture().await;
    fixture
        .state
        .missing_incarnation
        .store(true, Ordering::SeqCst);
    resources(&fixture).await.unwrap();
    fixture.drained().await;
    resources(&fixture).await.unwrap();
    fixture.drained().await;
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.state.pod_reads.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn native_http_poisoned_stream_cannot_pin_a_replaced_container_forever() {
    let fixture = fixture().await;
    resources(&fixture).await.unwrap();
    fixture.state.fault.store(1, Ordering::SeqCst);
    assert!(resources(&fixture).await.is_err());
    fixture.drained().await;
    fixture.state.restarted.store(true, Ordering::SeqCst);
    assert!(
        resources(&fixture).await.is_err(),
        "old hint must fail before opening/sending"
    );
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    assert!(fixture.transport.state.lock().await.native_http.is_empty());
    resources(&fixture).await.unwrap();
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 3);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn status_capability_opt_in_is_present_on_new_and_reused_native_http() {
    let fixture = fixture().await;
    let request = crate::RuntimeStatusRequest {
        sandbox_id: "sandbox-one".into(),
        generation: 1,
        home_identity: "a".repeat(43),
    };
    let fresh = fixture
        .transport
        .exchange(&handle(), RequestFrame::RuntimeStatus(request.clone()))
        .await
        .unwrap();
    let endpoint = fixture.endpoint().await;
    let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
    let lease = fixture
        .transport
        .native_checkout(&endpoint, slot)
        .await
        .unwrap();
    let reused = fixture
        .transport
        .status_on_native_lease(&endpoint, &request, lease)
        .await
        .unwrap();
    assert_eq!(fresh, reused);
    let ResponseFrame::RuntimeStatus(status) = reused else {
        panic!("expected status")
    };
    assert!(
        !status.agent_resource_preparation,
        "retained runtime did not advertise support"
    );
    assert!(!status.exact_github_credential_route);
    let requests = fixture.state.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    for received in requests {
        assert!(received.to_ascii_lowercase().contains(&format!(
            "accept: {}, {}, {}, {}, {}, {}",
            crate::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE,
            crate::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE,
            crate::wire::AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE,
            crate::wire::AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE,
            crate::wire::GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE,
            crate::wire::GITHUB_REPOSITORY_SCOPE_STATUS_MEDIA_TYPE,
        )));
    }
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}
