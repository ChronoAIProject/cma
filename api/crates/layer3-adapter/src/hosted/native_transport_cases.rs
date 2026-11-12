use super::*;

#[tokio::test]
async fn native_api_stream_carries_authenticated_control_history_and_websocket() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::Success, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        assert_eq!(
            fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources)
                .await
                .unwrap(),
            ResponseFrame::Resources(ResourceReading::default())
        );
        assert_eq!(
            fixture
                .transport
                .export_agent_history(&handle(), &AgentSlot::new("agent-a"))
                .await
                .unwrap(),
            b"{\"history\":[]}"
        );
        let mut socket = fixture
            .transport
            .connect(&endpoint, &AgentSlot::new("agent-a"))
            .await
            .unwrap();
        socket
            .send(Message::Binary(vec![0x02, 1, 2, 3]))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(vec![0x02, 1, 2, 3])
        );
        drop(socket);
        fixture.drained().await;
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_api_stream_rejects_replaced_pod_before_open_and_wrong_ownership() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        for ownership in [false, true] {
            let fixture = Fixture::start(Mode::Success, transport_mode).await;
            let endpoint = fixture.endpoint().await;
            if ownership {
                fixture.state.wrong_owner.store(true, Ordering::SeqCst);
            } else {
                fixture.state.replaced.store(true, Ordering::SeqCst);
            }
            assert!(fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources)
                .await
                .is_err());
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 0);
            assert!(fixture.state.requests.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn native_api_stream_rejects_replacement_during_open_before_sending_token() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::ReplaceOnOpen, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        assert!(fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap_err()
            .to_string()
            .contains("Pod changed"));
        fixture.drained().await;
        assert!(fixture.state.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_api_stream_scrubs_refusals_channel_errors_and_runtime_error_bodies() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        for mode in [
            Mode::Forbidden,
            Mode::Throttled,
            Mode::ErrorChannel,
            Mode::HttpError,
        ] {
            let fixture = Fixture::start(mode, transport_mode).await;
            let endpoint = fixture.endpoint().await;
            let error = fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources)
                .await
                .unwrap_err();
            let text = format!("{error:?}");
            assert!(!text.contains(ERROR_SECRET));
            assert!(!text.contains(API_TOKEN));
            assert!(!text.contains(endpoint.control_token().unwrap()));
            if matches!(mode, Mode::Forbidden) {
                assert!(matches!(error, AdapterError::Rejected(_)));
            }
            if matches!(mode, Mode::Throttled) {
                assert!(matches!(error, AdapterError::AtCapacity(_)));
            }
            fixture.drained().await;
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn native_api_stream_never_retries_a_control_request_after_lost_response() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::LostResponse, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let mutation = RequestFrame::Install(crate::InstallRequest::new(
            "install-once",
            vec!["echo side-effect".into()],
        ));
        assert!(fixture
            .transport
            .control(&endpoint, &mutation)
            .await
            .is_err());
        fixture.drained().await;
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
        assert!(fixture.state.requests.lock().unwrap()[0].contains("install-once"));
    }
}

#[tokio::test]
async fn native_api_stream_cancellation_and_oversized_chunked_body_close_forwarder() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        for mode in [Mode::Stall, Mode::StallAfterOpen, Mode::LargeBody] {
            let fixture = Fixture::start(mode, transport_mode).await;
            let endpoint = fixture.endpoint().await;
            let operation = fixture.transport.native_http(
                &endpoint,
                http::Method::POST,
                endpoint.control().unwrap(),
                b"{}".to_vec(),
                32,
            );
            let result = tokio::time::timeout(Duration::from_millis(250), operation).await;
            if matches!(mode, Mode::Stall | Mode::StallAfterOpen) {
                assert!(result.is_err());
            } else {
                assert!(matches!(result.unwrap(), Err(AdapterError::Rejected(_))));
            }
            fixture.drained().await;
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn native_api_stream_scrubs_runtime_token_from_typed_failure() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::WireError, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let response = fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap();
        let ResponseFrame::Failure(failure) = response else {
            panic!("expected typed failure");
        };
        assert_eq!(failure.detail, "echoed credential [redacted]");
        fixture.drained().await;
    }
}

#[tokio::test]
async fn native_api_stream_silent_upstream_close_refuses_further_writes() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::SilentClose, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let stream = fixture
            .transport
            .native_stream(&endpoint, RuntimeStreamKind::Conversation)
            .await;
        let Ok(mut stream) = stream else {
            assert_eq!(transport_mode, NativeRuntimeTransport::KubernetesExec);
            fixture.drained().await;
            continue;
        };
        let mut bytes = [0; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut bytes))
            .await
            .unwrap();
        assert!(closed.is_err() || closed.unwrap() == 0);
        assert!(stream
            .write_all(b"must-not-reach-a-closed-runtime")
            .await
            .is_err());
        drop(stream);
        fixture.drained().await;
        assert!(fixture.state.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_api_stream_conversations_have_a_shared_bound_and_reserve_control_capacity() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::Success, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let mut streams = Vec::new();
        for _ in 0..64 {
            streams.push(
                fixture
                    .transport
                    .native_stream(&endpoint, RuntimeStreamKind::Conversation)
                    .await
                    .unwrap(),
            );
        }
        assert!(matches!(
            fixture
                .transport
                .native_stream(&endpoint, RuntimeStreamKind::Conversation)
                .await,
            Err(AdapterError::AtCapacity(_))
        ));
        fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap();
        drop(streams.pop());
        streams.push(
            fixture
                .transport
                .native_stream(&endpoint, RuntimeStreamKind::Conversation)
                .await
                .unwrap(),
        );
        drop(streams);
        fixture.drained().await;
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 66);
    }
}

#[tokio::test]
async fn native_exec_success_status_preserves_buffered_stdout_and_refuses_writes() {
    let fixture = Fixture::start(Mode::BufferedExit, NativeRuntimeTransport::KubernetesExec).await;
    let endpoint = fixture.endpoint().await;
    let mut stream = fixture
        .transport
        .native_stream(&endpoint, RuntimeStreamKind::Request)
        .await
        .unwrap();
    fixture.drained().await;
    // Let the kube driver observe the exit while the caller deliberately delays reading.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(stream.write_all(b"after-exit").await.is_err());
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"buffered-response");
    assert!(fixture.state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_exec_history_drains_large_buffered_response_after_successful_exit() {
    let fixture = Fixture::start(
        Mode::BufferedHistoryExit,
        NativeRuntimeTransport::KubernetesExec,
    )
    .await;
    let expected = "{\"history\":\"".to_owned() + &"x".repeat(48 * 1024) + "\"}\n";
    for _ in 0..8 {
        let history = fixture
            .transport
            .export_agent_history(&handle(), &AgentSlot::new("agent-a"))
            .await
            .unwrap();
        assert_eq!(history, expected.as_bytes());
    }
    fixture.drained().await;
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn native_exec_readiness_failure_and_cancellation_never_send_runtime_credentials() {
    for mode in [Mode::StallBeforeReady, Mode::InvalidReady] {
        let fixture = Fixture::start(mode, NativeRuntimeTransport::KubernetesExec).await;
        let endpoint = fixture.endpoint().await;
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources),
        )
        .await;
        if matches!(mode, Mode::StallBeforeReady) {
            assert!(result.is_err());
        } else {
            assert!(matches!(result.unwrap(), Err(AdapterError::Rejected(_))));
        }
        fixture.drained().await;
        assert!(fixture.state.requests.lock().unwrap().is_empty());
    }
}
