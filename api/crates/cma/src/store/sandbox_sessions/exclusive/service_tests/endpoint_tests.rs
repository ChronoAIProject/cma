//! Real CMA auth/public routes, concrete hosted adapter and runtime receiver.
//! IdP, provider proxy and display are local fixtures; no live-provider/image claim.
use super::ingress::{Ingress, ORIGIN};
use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{
    tungstenite::{Error, Message},
    MaybeTlsStream, WebSocketStream,
};

mod child;
mod fixture;
mod measurements;
mod provider;
use fixture::Fixture;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn echo(socket: &mut Socket, value: u8) {
    socket.send(Message::Binary(vec![value])).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reply, Message::Binary(vec![value]));
}

async fn closed(socket: &mut Socket) {
    let reply = tokio::time::timeout(Duration::from_secs(6), socket.next())
        .await
        .unwrap();
    assert!(matches!(
        reply,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
}

fn refusal(result: Result<Socket, Error>, expected: u16) {
    match result {
        Err(Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), expected);
            if [409, 503].contains(&expected) {
                assert_eq!(response.headers()["retry-after"], "1");
                assert_eq!(response.headers()["cache-control"], "no-store, private");
            }
        }
        _ => panic!("expected a typed public refusal"),
    }
}

#[tokio::test]
#[ignore = "requires dedicated desktop-replica fixture runner and disposable Mongo"]
async fn public_disabled_services_and_ownerless_receiver_refuse_new_streams() {
    let f = Fixture::new(false).await;
    for ingress in 0..2 {
        let response = f.discovery(ingress).await;
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["retry-after"], "1");
        assert_eq!(response.headers()["cache-control"], "no-store, private");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], "desktop_authority_unavailable");
    }
    let (left, right) = tokio::join!(f.connect(0), f.connect(1));
    for result in [left, right] {
        match result {
            Err(Error::Http(response)) => {
                assert_eq!(response.status(), 503);
                assert_eq!(response.headers()["retry-after"], "1");
                assert_eq!(response.headers()["cache-control"], "no-store, private");
                let body: serde_json::Value =
                    serde_json::from_slice(response.body().as_ref().unwrap()).unwrap();
                assert_eq!(body["code"], "desktop_authority_unavailable");
            }
            _ => panic!("disabled authority must refuse before upgrade"),
        }
    }
    assert_eq!(f.proxy.state.opens.load(Ordering::SeqCst), 0);
    assert!(f.proxy.state.confirmations.lock().unwrap().is_empty());
    assert_eq!(
        f.resources
            .stores
            .db
            .collection::<mongodb::bson::Document>("sandbox_sessions")
            .count_documents(mongodb::bson::doc! {})
            .await
            .unwrap(),
        0
    );
    let ownerless = tokio_tungstenite::connect_async(f.raw_receiver_request(None)).await;
    assert!(matches!(ownerless, Err(Error::Http(response)) if response.status() == 400));
    assert!(!f.root.path.join("state/desktop-owner/floor.json").exists());
    assert!(!f.root.path.join("state/desktop-owner.activated").exists());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires dedicated desktop-replica fixture runner and disposable Mongo"]
async fn public_replica_handoff_and_revocation_use_actual_receiver() {
    let f = Fixture::new(true).await;
    let anonymous = f.ingresses[0]
        .discovery(&f.resources.agents[0], &Default::default())
        .await;
    assert_eq!(anonymous.status(), 401);
    assert_eq!(f.proxy.state.opens.load(Ordering::SeqCst), 0);
    assert_eq!(f.discovery(0).await.status(), 200);
    let (left, right) = tokio::join!(f.connect(0), f.connect(1));
    let (mut first, winner) = match (left, right) {
        (Ok(socket), refused) => {
            refusal(refused, 409);
            (socket, 0)
        }
        (refused, Ok(socket)) => {
            refusal(refused, 409);
            (socket, 1)
        }
        _ => panic!("one public ingress must own the shared Sandbox"),
    };
    assert_eq!(f.proxy.state.opens.load(Ordering::SeqCst), 1);
    let original = f.proxy.last_owner();
    echo(&mut first, 1).await;
    let contender = 1 - winner;
    let busy = f.discovery(contender).await;
    assert_eq!(busy.status(), 409);
    assert_eq!(busy.headers()["retry-after"], "1");
    // Multiple real receiver renewals retain the exact epoch beyond its initial
    // ten-second receiver lifetime. Frames are returned, not just sent.
    for value in 2..=13 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        echo(&mut first, value).await;
    }
    assert_eq!(f.proxy.last_owner().epoch, original.epoch);
    assert!(f.proxy.last_owner().expires_at_ms > original.expires_at_ms);
    first.close(None).await.unwrap();
    f.available(contender, Duration::from_secs(5)).await;
    let mut successor = f.connect(contender).await.unwrap();
    echo(&mut successor, 20).await;
    let next = f.proxy.last_owner();
    assert!(next.epoch > original.epoch);
    assert_ne!(next.session_id, original.session_id);
    let calls = f.auth.provider_snapshot();
    assert!(calls.userinfo.completed > 2 && calls.introspection.completed > 2);
    assert_eq!(calls.userinfo.timed, 0);
    f.auth.deny_provider(true);
    closed(&mut successor).await;
    f.auth.deny_provider(false);
    f.available(winner, Duration::from_secs(5)).await;
    let mut third = f.connect(winner).await.unwrap();
    echo(&mut third, 21).await;
    f.auth.revoke_login().await;
    closed(&mut third).await;
    assert_eq!(f.discovery(contender).await.status(), 401);
    refusal(f.connect(contender).await, 401);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires dedicated desktop-replica fixture runner and disposable Mongo"]
async fn public_uncertain_confirm_and_receiver_restart_preserve_fencing() {
    let mut f = Fixture::new(true).await;
    f.proxy.state.lose_confirm_ack.store(true, Ordering::SeqCst);
    refusal(f.connect(0).await, 503);
    let original = f.proxy.last_owner();
    assert_eq!(f.proxy.state.opens.load(Ordering::SeqCst), 0);
    assert_eq!(f.discovery(1).await.status(), 409);
    assert_eq!(f.root.floor()["owner"]["epoch"], original.epoch);
    f.restart_receiver().await;
    assert_eq!(f.root.floor()["owner"]["epoch"], original.epoch);
    // Boot changed with the same durable floor. Neither old authority nor
    // legacy admission can reopen this actual receiver.
    assert!(
        tokio_tungstenite::connect_async(f.raw_receiver_request(None))
            .await
            .is_err()
    );
    assert!(
        tokio_tungstenite::connect_async(f.raw_receiver_request(Some(&original)))
            .await
            .is_err()
    );
    assert_eq!(f.discovery(1).await.status(), 409);
    f.proxy
        .state
        .lose_confirm_ack
        .store(false, Ordering::SeqCst);
    // Preserve the real 15-second Mongo lease; do not mutate expiry for this case.
    f.available(1, Duration::from_secs(17)).await;
    let mut successor = f.connect(1).await.unwrap();
    echo(&mut successor, 30).await;
    assert!(f.proxy.last_owner().epoch > original.epoch);
    assert!(
        tokio_tungstenite::connect_async(f.raw_receiver_request(Some(&original)))
            .await
            .is_err()
    );
    echo(&mut successor, 31).await;
    f.proxy.state.lose_revoke_ack.store(true, Ordering::SeqCst);
    successor.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.proxy.state.active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.discovery(0).await.status(), 409);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires dedicated desktop-replica fixture runner and disposable Mongo"]
async fn delayed_receiver_renewal_ack_cannot_restore_public_authority() {
    let f = Fixture::new(true).await;
    let started = std::time::Instant::now();
    let mut first = f.connect(0).await.unwrap();
    let original = f.proxy.last_owner();
    f.proxy.state.hold_renew_ack.store(true, Ordering::SeqCst);
    tokio::time::timeout(
        Duration::from_secs(5),
        f.proxy.state.renewal_entered.notified(),
    )
    .await
    .unwrap();
    echo(&mut first, 40).await;
    let reply = tokio::time::timeout(Duration::from_secs(11), first.next())
        .await
        .unwrap();
    assert!(matches!(
        reply,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
    assert!(
        started.elapsed() >= Duration::from_secs(9),
        "fixture must exercise original expiry"
    );
    f.proxy.state.hold_renew_ack.store(false, Ordering::SeqCst);
    f.available(1, Duration::from_secs(17)).await;
    let mut successor = f.connect(1).await.unwrap();
    let next = f.proxy.last_owner();
    assert!(next.epoch > original.epoch);
    f.proxy.state.renewal_release.notify_one();
    echo(&mut successor, 41).await;
    assert!(
        tokio_tungstenite::connect_async(f.raw_receiver_request(Some(&original)))
            .await
            .is_err()
    );
    successor.close(None).await.unwrap();
    f.available(0, Duration::from_secs(5)).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires dedicated desktop-replica fixture runner and disposable Mongo"]
async fn retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor() {
    for expires in [false, true] {
        let f = Fixture::new(true).await;
        let id = &f.resources.stores.request.sandbox_id;
        let expiry = layer2_core::rfc3339_at(
            std::time::SystemTime::now() + Duration::from_secs(if expires { 2 } else { 90 }),
        );
        f.resources
            .stores
            .left
            .open_sandbox_session(id, "retained-legacy", &expiry)
            .await
            .unwrap();
        assert!(f
            .resources
            .stores
            .right
            .has_open_sandbox_session(id, &layer2_core::now_rfc3339())
            .await
            .unwrap());
        assert_eq!(f.discovery(0).await.status(), 409);
        refusal(f.connect(1).await, 409);
        assert_eq!(f.proxy.state.opens.load(Ordering::SeqCst), 0);
        if !expires {
            f.resources
                .left
                .close(id.clone(), "retained-legacy".into())
                .await;
        }
        f.available(0, Duration::from_secs(5)).await;
        let mut successor = f.connect(0).await.unwrap();
        echo(&mut successor, 30).await;
        let owner = f.proxy.last_owner();
        assert_ne!(owner.session_id, "retained-legacy");
        let sessions = f
            .resources
            .stores
            .db
            .collection::<mongodb::bson::Document>("sandbox_sessions");
        let query = mongodb::bson::doc! { "_id": &owner.session_id };
        let before = sessions.find_one(query.clone()).await.unwrap().unwrap();
        let _ = f
            .resources
            .stores
            .right
            .renew_sandbox_session(id, "retained-legacy", &expiry)
            .await;
        f.resources
            .right
            .close(id.clone(), "retained-legacy".into())
            .await;
        assert_eq!(sessions.find_one(query).await.unwrap().unwrap(), before);
        echo(&mut successor, 31).await;
        successor.close(None).await.unwrap();
        f.available(1, Duration::from_secs(5)).await;
        f.cleanup().await;
    }
}
