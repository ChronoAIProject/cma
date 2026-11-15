//! Actual HTTP/WebSocket routers, shared Mongo, and a fixture runtime receiver.
use super::ingress::{Ingress, ORIGIN};
use super::*;
use axum::{http::HeaderMap, Extension};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Error, Message};

async fn modeled_ingress(service: DesktopService, fixture: &Services, proof: bool) -> Ingress {
    let mut app = layer1_public::rest::desktop_router(Arc::new(service), ORIGIN.into())
        .layer(Extension(layer1_public::AuthenticatedSubject(
            fixture.caller.subject.clone(),
        )))
        .layer(Extension(fixture.caller.authorization.clone()));
    if proof {
        app = app.layer(Extension(fixture.authority()));
    }
    Ingress::start(app).await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_public_ingresses_handoff_one_sandbox_socket_and_revoke_on_remote_change() {
    let f = Services::new(true).await;
    f.receiver.echo_frames();
    let left = modeled_ingress(f.left.clone(), &f, true).await;
    let right = modeled_ingress(f.right.clone(), &f, true).await;
    let headers = HeaderMap::new();
    assert_eq!(left.discovery(&f.agents[0], &headers).await.status(), 200);
    let refused = tokio_tungstenite::connect_async(left.websocket(
        &f.agents[0],
        "http://other.test",
        &headers,
    ))
    .await
    .unwrap_err();
    assert!(matches!(refused, Error::Http(response) if response.status() == 403));

    let (mut first, _) =
        tokio_tungstenite::connect_async(left.websocket(&f.agents[0], ORIGIN, &headers))
            .await
            .unwrap();
    first.send(Message::Binary(vec![1, 2])).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), first.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Binary(vec![1, 2])
    );
    let busy = right.discovery(&f.agents[1], &headers).await;
    assert_eq!(busy.status(), 409);
    assert_eq!(busy.headers()["retry-after"], "1");
    assert_eq!(busy.headers()["cache-control"], "no-store, private");
    let refused = tokio_tungstenite::connect_async(right.websocket(&f.agents[1], ORIGIN, &headers))
        .await
        .unwrap_err();
    assert!(matches!(refused, Error::Http(response) if response.status() == 409));
    first.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while right.discovery(&f.agents[1], &headers).await.status() != 200 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let (mut next, _) =
        tokio_tungstenite::connect_async(right.websocket(&f.agents[1], ORIGIN, &headers))
            .await
            .unwrap();
    next.send(Message::Text("fixture-frame".into()))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), next.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Text("fixture-frame".into())
    );
    f.stop_agent(1).await;
    let closed = tokio::time::timeout(Duration::from_secs(5), next.next())
        .await
        .unwrap();
    assert!(matches!(closed, Some(Ok(Message::Close(_))) | None));
    assert_eq!(f.receiver.frames(), 2);
    assert_eq!(left.discovery(&f.agents[0], &headers).await.status(), 200);
    left.stop().await;
    right.stop().await;
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn replica_public_ingress_refuses_missing_current_credential_proof() {
    let f = Services::new(true).await;
    let ingress = modeled_ingress(f.left.clone(), &f, false).await;
    let headers = HeaderMap::new();
    let refused =
        tokio_tungstenite::connect_async(ingress.websocket(&f.agents[0], ORIGIN, &headers))
            .await
            .unwrap_err();
    match refused {
        Error::Http(response) => {
            assert_eq!(response.status(), 503);
            assert_eq!(response.headers()["retry-after"], "1");
        }
        _ => panic!("missing renewable proof must produce a typed refusal"),
    }
    assert_eq!(f.receiver.frames(), 0);
    assert_eq!(
        ingress.discovery(&f.agents[0], &headers).await.status(),
        200
    );
    ingress.stop().await;
    f.stores.db.drop().await.unwrap();
}
