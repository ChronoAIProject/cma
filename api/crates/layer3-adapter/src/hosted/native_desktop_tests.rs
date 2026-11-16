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
async fn owned_native_direct_is_unsupported_before_discovery_or_credentials() {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::Direct).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "logical-sandbox", &"a".repeat(64))
        .await
        .unwrap();
    assert!(matches!(
        DesktopOwnedRuntime::challenge(&*fixture.transport, &handle()).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert_eq!(fixture.state.pod_reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.runtime_bytes.load(Ordering::SeqCst), 0);
    assert!(fixture.state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn changed_authority_during_desktop_preparation_sends_no_credentials() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        for fault in ["assignment", "retire", "pod", "container", "cancel"] {
            let fixture = Fixture::start(Mode::Persistent, mode).await;
            fixture
                .transport
                .bind_native_runtime(&handle(), "logical-sandbox", &"a".repeat(64))
                .await
                .unwrap();
            let challenge = DesktopOwnedRuntime::challenge(&*fixture.transport, &handle())
                .await
                .unwrap();
            fixture
                .state
                .hold_next_upgrade
                .store(true, Ordering::SeqCst);
            let transport = fixture.transport.clone();
            let opening = tokio::spawn(async move {
                DesktopOwnedRuntime::open(&*transport, &handle(), &owner(), &challenge).await
            });
            tokio::time::timeout(
                Duration::from_secs(2),
                fixture.state.preparation_entered.notified(),
            )
            .await
            .expect("WebSocket preparation follows completed confirmation");
            assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
            let bytes = fixture.state.runtime_bytes.load(Ordering::SeqCst);
            match fault {
                "assignment" => fixture
                    .transport
                    .bind_native_runtime(&handle(), "logical-sandbox", &"b".repeat(64))
                    .await
                    .unwrap(),
                "retire" => fixture.transport.retire(&handle()).await.unwrap(),
                "pod" => fixture.state.replaced.store(true, Ordering::SeqCst),
                "container" => fixture.state.restarted.store(true, Ordering::SeqCst),
                _ => opening.abort(),
            }
            fixture.state.preparation_release.notify_one();
            let result = tokio::time::timeout(Duration::from_secs(2), opening)
                .await
                .unwrap();
            if fault == "cancel" {
                assert!(result.is_err_and(|error| error.is_cancelled()));
            } else {
                assert!(result.is_ok_and(|result| result.is_err()));
            }
            fixture.transport.retire(&handle()).await.unwrap();
            fixture.drained().await;
            assert_eq!(
                fixture.state.runtime_bytes.load(Ordering::SeqCst),
                bytes,
                "no authenticated bytes after {fault} during {mode:?} preparation"
            );
            assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
        }
    }
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

pub(super) async fn pause_preparation(state: &Arc<State>) -> Option<Active> {
    if !state.hold_next_upgrade.swap(false, Ordering::SeqCst) {
        return None;
    }
    state.active.fetch_add(1, Ordering::SeqCst);
    let guard = Active(state.clone());
    state.preparation_entered.notify_one();
    state.preparation_release.notified().await;
    Some(guard)
}
