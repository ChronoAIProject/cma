//! Actual HTTP/WebSocket routers, shared Mongo, and a fixture runtime receiver.
use super::*;
use axum::Extension;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error, Message};

const ORIGIN: &str = "http://desktop.example.test";

struct Ingress {
    address: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Ingress {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Ingress {
    async fn start(service: DesktopService, fixture: &Services, proof: bool) -> Self {
        let mut app = layer1_public::rest::desktop_router(Arc::new(service), ORIGIN.into())
            .layer(Extension(layer1_public::AuthenticatedSubject(
                fixture.caller.subject.clone(),
            )))
            .layer(Extension(fixture.caller.authorization.clone()));
        if proof {
            app = app.layer(Extension(fixture.authority()));
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { address, task }
    }

    fn websocket(&self, id: &AgentId, origin: &str) -> axum::http::Request<()> {
        let mut request = format!("ws://{}/api/v1/agents/{id}/desktop/ws", self.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        request
    }

    async fn discovery(&self, id: &AgentId) -> reqwest::Response {
        reqwest::get(format!(
            "http://{}/api/v1/agents/{id}/desktop",
            self.address
        ))
        .await
        .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_public_ingresses_handoff_one_sandbox_socket_and_revoke_on_remote_change() {
    let f = Services::new(true).await;
    f.receiver.echo_frames();
    let left = Ingress::start(f.left.clone(), &f, true).await;
    let right = Ingress::start(f.right.clone(), &f, true).await;
    assert_eq!(left.discovery(&f.agents[0]).await.status(), 200);
    let refused =
        tokio_tungstenite::connect_async(left.websocket(&f.agents[0], "http://other.test"))
            .await
            .unwrap_err();
    assert!(matches!(refused, Error::Http(response) if response.status() == 403));

    let (mut first, _) = tokio_tungstenite::connect_async(left.websocket(&f.agents[0], ORIGIN))
        .await
        .unwrap();
    first
        .send(Message::Binary(vec![1, 2].into()))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), first.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Binary(vec![1, 2].into())
    );
    let busy = right.discovery(&f.agents[1]).await;
    assert_eq!(busy.status(), 409);
    assert_eq!(busy.headers()["retry-after"], "1");
    assert_eq!(busy.headers()["cache-control"], "no-store, private");
    let refused = tokio_tungstenite::connect_async(right.websocket(&f.agents[1], ORIGIN))
        .await
        .unwrap_err();
    assert!(matches!(refused, Error::Http(response) if response.status() == 409));
    first.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while right.discovery(&f.agents[1]).await.status() != 200 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let (mut next, _) = tokio_tungstenite::connect_async(right.websocket(&f.agents[1], ORIGIN))
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
    assert_eq!(left.discovery(&f.agents[0]).await.status(), 200);
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn replica_public_ingress_refuses_missing_current_credential_proof() {
    let f = Services::new(true).await;
    let ingress = Ingress::start(f.left.clone(), &f, false).await;
    let refused = tokio_tungstenite::connect_async(ingress.websocket(&f.agents[0], ORIGIN))
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
    assert_eq!(ingress.discovery(&f.agents[0]).await.status(), 200);
    f.stores.db.drop().await.unwrap();
}
