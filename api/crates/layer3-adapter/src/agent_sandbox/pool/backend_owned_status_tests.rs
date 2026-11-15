use super::*;

async fn owned_fixture() -> ReusedInspection {
    let mut fixture = fixture(false, true).await;
    let (socket, _) = fixture
        .transport
        .seed_native_ownership_connection(&fixture.bound.runtime)
        .await;
    fixture.ownership_task = Some(tokio::spawn(serve(
        socket,
        fixture.stream.clone(),
        fixture.bound.fixture.state.clone(),
    )));
    fixture
}

pub(super) async fn write_owner_response(socket: &mut tokio::io::DuplexStream, body: &[u8]) {
    socket
        .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    socket.write_all(body).await.unwrap();
}

#[tokio::test]
async fn native_owned_status_negotiates_receiver_before_reused_status() {
    use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};
    let fixture = owned_fixture().await;
    let authority = TestAuthority::new(2);
    *fixture.stream.owner.lock().unwrap() = Some(authority.owner());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        scope(
            authority.clone(),
            fixture.adapter.inspect_runtime(&fixture.adoption),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert!(result.status.unwrap().credential_ready);
    assert_eq!(
        &*fixture.stream.owner_events.lock().unwrap(),
        &["challenge", "confirm", "status"]
    );
    assert_eq!(authority.renewals.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
    assert!(authority.is_current());
}

#[tokio::test]
async fn native_owned_status_refused_receiver_never_sends_status() {
    use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};
    let fixture = owned_fixture().await;
    let authority = TestAuthority::new(2);
    *fixture.stream.owner.lock().unwrap() = Some(authority.owner());
    fixture
        .stream
        .refuse_confirmation
        .store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        scope(
            authority.clone(),
            fixture.adapter.inspect_runtime(&fixture.adoption),
        ),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(
        &*fixture.stream.owner_events.lock().unwrap(),
        &["challenge", "confirm"]
    );
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 0);
    assert!(!authority.is_current());
}

#[tokio::test]
async fn reused_unowned_status_pod_mismatch_cancels_old_ownership_lane() {
    for pointer in ["/metadata/uid", "/status/containerStatuses/0/containerID"] {
        let fixture = fixture(false, true).await;
        let (_socket, cancelled) = fixture
            .transport
            .seed_native_ownership_connection(&fixture.bound.runtime)
            .await;
        *fixture
            .bound
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .get_mut(POD)
            .unwrap()
            .pointer_mut(pointer)
            .unwrap() = json!("replacement");
        // Bootstrap the replacement using the real backend so its fresh graph
        // proof is acknowledged, while the prior incarnation remains cached.
        fixture
            .backend
            .adopt(&fixture.bound.runtime, &fixture.bound.request)
            .await
            .unwrap()
            .unwrap();
        assert!(!*cancelled.borrow(), "exercise status's mismatch branch");
        assert!(fixture
            .backend
            .inspect_native_runtime(
                &fixture.bound.runtime,
                &fixture.bound.request,
                &fixture.home,
            )
            .await
            .is_err());
        assert!(
            *cancelled.borrow(),
            "authoritative Control proof must fence old Ownership too"
        );
        assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 0);
        fixture
            .transport
            .retire(&fixture.bound.runtime)
            .await
            .unwrap();
    }
}
