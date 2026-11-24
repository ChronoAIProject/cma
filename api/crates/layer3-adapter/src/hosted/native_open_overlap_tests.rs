//! Real nested HTTP/WebSocket traffic verifies the preparation/authorization boundary.
use super::*;
use crate::hosted::HostedControlTransport;

#[path = "native_open_receiver_tests.rs"]
mod receiver;

async fn fixture(transport_mode: NativeRuntimeTransport) -> Fixture {
    let fixture = Fixture::start(Mode::Persistent, transport_mode).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"a".repeat(64))
        .await
        .unwrap();
    fixture
        .transport
        .exchange(&handle(), RequestFrame::Resources)
        .await
        .unwrap();
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    fixture
}

fn launch() -> FencedRequest<AgentLaunch> {
    FencedRequest::new(
        9,
        AgentLaunch {
            operation_id: "agent:agent-a:open:v1".into(),
            request_sha256: "b".repeat(64),
            agent_id: AgentSlot::new("agent-a"),
            cwd: "/home/cma/workspaces/worktree".into(),
            home: "/home/cma/agents/agent-a".into(),
            program: "codex-acp".into(),
            args: vec![],
            env: BTreeMap::new(),
        },
    )
}

fn start(
    fixture: &Fixture,
) -> tokio::task::JoinHandle<crate::Result<(FencedReceipt<AgentOpenReceipt>, Socket)>> {
    let transport = fixture.transport.clone();
    tokio::spawn(async move {
        let endpoint = transport.endpoint(&handle()).await?;
        transport
            .open_and_connect(
                &handle(),
                &endpoint,
                &launch(),
                None,
                &mut crate::hosted::startup_timing::StartupTiming::new(),
                RuntimeWorkloadClass::Ordinary,
            )
            .await
    })
}

async fn overlapping(fixture: &Fixture, accepted: bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.opened.load(Ordering::SeqCst) != 2
            || fixture.state.requests.lock().unwrap().len() != 2
            || (accepted && fixture.state.active.load(Ordering::SeqCst) != 2)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("raw conversation stream prepares while actual Open is held");
    let requests = fixture.state.requests.lock().unwrap();
    assert!(requests[1].contains("agent:agent-a:open:v1"));
    assert!(requests
        .iter()
        .all(|request| !request.to_ascii_lowercase().contains("upgrade: websocket")));
}

#[tokio::test]
async fn native_open_prepares_stream_before_receipt_but_sends_websocket_only_after_receipt() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        let fixture = fixture(mode).await;
        let opening = start(&fixture);
        overlapping(&fixture, true).await;
        assert!(!opening.is_finished());
        let observations = fixture.state.pod_reads.load(Ordering::SeqCst);
        fixture.state.hold_response.store(false, Ordering::SeqCst);
        let (receipt, mut socket) = opening.await.unwrap().unwrap();
        assert_eq!(receipt.authority_fence, 9);
        assert_eq!(receipt.receipt.operation_id, launch().request.operation_id);
        assert_eq!(
            fixture.state.pod_reads.load(Ordering::SeqCst),
            observations + 1,
            "a fresh Pod observation follows the actual Open receipt"
        );
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 3);
        socket.send(Message::Binary(vec![2, 4, 6])).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(vec![2, 4, 6])
        );
        drop(socket);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn native_open_rechecks_replacement_after_receipt_before_websocket_credentials() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        let fixture = fixture(mode).await;
        let opening = start(&fixture);
        overlapping(&fixture, true).await;
        fixture.state.replaced.store(true, Ordering::SeqCst);
        fixture.state.hold_response.store(false, Ordering::SeqCst);
        assert!(matches!(
            opening.await.unwrap(),
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn native_open_observes_mutation_result_even_when_preparation_is_rejected() {
    let fixture = fixture(NativeRuntimeTransport::KubernetesExec).await;
    fixture
        .state
        .reject_next_upgrade
        .store(true, Ordering::SeqCst);
    let opening = start(&fixture);
    overlapping(&fixture, false).await;
    // Preparation has failed, but the already sent mutation must remain awaited.
    assert!(tokio::time::timeout(Duration::from_millis(30), async {
        while !opening.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_err());
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    assert!(matches!(
        opening.await.unwrap(),
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_open_lost_response_is_not_replayed_and_cancels_prepared_stream() {
    let fixture = fixture(NativeRuntimeTransport::KubernetesExec).await;
    let opening = start(&fixture);
    overlapping(&fixture, true).await;
    fixture.state.fault.store(1, Ordering::SeqCst);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    assert!(matches!(
        opening.await.unwrap(),
        Err(AdapterError::Unavailable(_))
    ));
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_open_cancellation_closes_prepared_stream_without_websocket_credentials() {
    let fixture = fixture(NativeRuntimeTransport::KubernetesExec).await;
    let opening = start(&fixture);
    overlapping(&fixture, true).await;
    opening.abort();
    let Err(cancelled) = opening.await else {
        panic!("opening was not cancelled");
    };
    assert!(cancelled.is_cancelled());
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_open_wrong_success_fence_cancels_prepared_stream_before_websocket() {
    let fixture = fixture(NativeRuntimeTransport::KubernetesExec).await;
    let opening = start(&fixture);
    overlapping(&fixture, true).await;
    fixture.state.fault.store(7, Ordering::SeqCst);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    assert!(matches!(
        opening.await.unwrap(),
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}
