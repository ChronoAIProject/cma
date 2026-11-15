use super::*;

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
    let fixture = fixture(false, true).await;
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
    let fixture = fixture(false, true).await;
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
