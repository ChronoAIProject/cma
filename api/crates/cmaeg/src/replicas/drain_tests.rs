use super::*;
use crate::http::readiness::Readiness;
use base64::{engine::general_purpose::STANDARD, Engine};

fn setup() -> (Control, Readiness) {
    use ring::rand::SecureRandom;
    let mut material = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut material).unwrap();
    let keys = Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode(material))).unwrap());
    let readiness = Readiness::initialized();
    let control = Control::new(
        keys,
        TurnTracker::new(readiness.clone()),
        8090,
        "https://fixture.invalid",
    );
    (control, readiness)
}

#[test]
fn drain_auth_binds_loopback_deployment_port_and_bounded_time() {
    let (control, _) = setup();
    let token = ticket(&control.keys, 8090, "https://fixture.invalid", 1000).unwrap();
    let peer = "127.0.0.1:42000".parse().unwrap();
    assert!(control.authorize(peer, &token, 1000));
    assert!(control.authorize(peer, &token, 1024));
    assert!(!control.authorize(peer, &token, 999));
    assert!(!control.authorize(peer, &token, 1025));
    assert!(!control.authorize("192.0.2.1:42000".parse().unwrap(), &token, 1000));
    for (port, origin) in [
        (8091, "https://fixture.invalid"),
        (8090, "https://other.invalid"),
    ] {
        let wrong = ticket(&control.keys, port, origin, 1000).unwrap();
        assert!(!control.authorize(peer, &wrong, 1000));
    }
    let (other, _) = setup();
    assert!(!other.authorize(peer, &token, 1000));
    assert!(!control.authorize(peer, "malformed", 1000));
}

#[tokio::test]
async fn drain_is_idempotent_and_readiness_never_returns() {
    let (control, readiness) = setup();
    assert!(readiness.report().ready);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), control.requested())
            .await
            .is_err()
    );
    control.start();
    control.requested().await;
    control.start();
    control.requested().await;
    assert!(!readiness.report().ready);
    assert!(control.tracker.is_draining());
}

#[tokio::test]
async fn http_drain_refuses_bad_auth_and_payload_then_accepts_retries() {
    let (control, readiness) = setup();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            control
                .router(true)
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .into_future(),
    );
    let client = reqwest::Client::new();
    let url = format!("http://{address}{PATH}");
    assert_eq!(client.post(&url).send().await.unwrap().status(), 401);
    assert!(readiness.report().ready);
    let token = ticket(
        &control.keys,
        8090,
        "https://fixture.invalid",
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
    .unwrap();
    assert_eq!(
        client
            .post(&url)
            .bearer_auth(&token)
            .body("unexpected")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert!(readiness.report().ready);
    for _ in 0..2 {
        assert_eq!(
            client
                .post(&url)
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
        assert!(!readiness.report().ready);
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

use std::future::IntoFuture;
