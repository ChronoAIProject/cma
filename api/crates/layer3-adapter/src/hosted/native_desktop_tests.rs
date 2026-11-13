use super::*;
use crate::desktop_ownership::DesktopOwnedRuntime;
use crate::hosted::HostedControlTransport;

fn owner() -> crate::DesktopOwnership {
    crate::DesktopOwnership {
        sandbox_id: "logical-sandbox".into(),
        runtime_id: handle().as_str().into(),
        session_id: "owned-desktop".into(),
        boot_id: None,
        epoch: 1,
        issued_at_ms: 1000,
        expires_at_ms: 16000,
    }
}

#[tokio::test]
async fn native_direct_socket_and_control_keep_pod_fencing_at_an_ephemeral_fixture_port() {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::Direct).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "logical-sandbox", &"a".repeat(64))
        .await
        .unwrap();
    let runtime = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = runtime.local_addr().unwrap();
    let state = fixture.state.clone();
    let token = state.runtimes.lock().unwrap()["claimed"].clone();
    let serving = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            let (stream, _) = runtime.accept().await.unwrap();
            let (state, token) = (state.clone(), token.clone());
            tasks.spawn(async move {
                serve_runtime(stream, Mode::Persistent, state, token).await;
            });
            while tasks.try_join_next().is_some() {}
        }
    });
    let challenge = super::super::super::desktop::tests::direct_challenge_at(
        &fixture.transport,
        &handle(),
        address,
    )
    .await;
    let mut connection =
        DesktopOwnedRuntime::open(&*fixture.transport, &handle(), &owner(), &challenge)
            .await
            .unwrap();
    connection
        .send(crate::DesktopFrame::Binary(vec![9]))
        .await
        .unwrap();
    assert_eq!(
        connection.recv().await.unwrap(),
        Some(crate::DesktopFrame::Binary(vec![9]))
    );
    let control = connection.ownership_control().unwrap();
    let challenge = control.ownership_challenge().await.unwrap();
    control.renew_ownership(&owner(), &challenge).await.unwrap();
    fixture.state.replaced.store(true, Ordering::SeqCst);
    let before = fixture.state.requests.lock().unwrap().len();
    assert!(control.ownership_challenge().await.is_err());
    assert!(control.revoke_ownership(&owner()).await.is_err());
    assert_eq!(fixture.state.requests.lock().unwrap().len(), before);
    drop(control);
    drop(connection);
    serving.abort();
    let _ = serving.await;
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn both_native_stream_modes_carry_owned_desktop_and_independent_control() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        let fixture = Fixture::start(Mode::Persistent, mode).await;
        fixture
            .transport
            .bind_native_runtime(&handle(), "logical-sandbox", &"a".repeat(64))
            .await
            .unwrap();
        let challenge = DesktopOwnedRuntime::challenge(&*fixture.transport, &handle())
            .await
            .unwrap();
        let mut connection =
            DesktopOwnedRuntime::open(&*fixture.transport, &handle(), &owner(), &challenge)
                .await
                .unwrap();
        connection
            .send(crate::DesktopFrame::Binary(vec![7, 8]))
            .await
            .unwrap();
        assert_eq!(
            connection.recv().await.unwrap(),
            Some(crate::DesktopFrame::Binary(vec![7, 8]))
        );
        let control = connection.ownership_control().unwrap();
        let next = control.ownership_challenge().await.unwrap();
        control.renew_ownership(&owner(), &next).await.unwrap();
        control.revoke_ownership(&owner()).await.unwrap();
        let requests = fixture.state.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 6);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.starts_with("GET /runtime/desktop "))
                .count(),
            1
        );
        assert!(!requests.iter().any(|r| r.contains("manager-ownership")));
        drop(control);
        drop(connection);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn native_pod_or_assignment_change_after_challenge_cannot_confirm_or_open() {
    for fault in ["pod", "boot", "assignment"] {
        let fixture =
            Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
        fixture
            .transport
            .bind_native_runtime(&handle(), "logical-sandbox", &"a".repeat(64))
            .await
            .unwrap();
        let challenge = DesktopOwnedRuntime::challenge(&*fixture.transport, &handle())
            .await
            .unwrap();
        match fault {
            "pod" => fixture.state.replaced.store(true, Ordering::SeqCst),
            "boot" => fixture.state.restarted.store(true, Ordering::SeqCst),
            _ => fixture
                .transport
                .bind_native_runtime(&handle(), "logical-sandbox", &"b".repeat(64))
                .await
                .unwrap(),
        }
        assert!(
            DesktopOwnedRuntime::open(&*fixture.transport, &handle(), &owner(), &challenge)
                .await
                .is_err()
        );
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn missing_native_assignment_is_refused_before_challenge_io() {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    fixture
        .transport
        .bind_runtime(&handle(), "logical-sandbox")
        .await
        .unwrap();
    assert!(
        DesktopOwnedRuntime::challenge(&*fixture.transport, &handle())
            .await
            .is_err()
    );
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 0);
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 0);
}
