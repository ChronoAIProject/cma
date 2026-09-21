use axum::serve;
use cmaeg::router;
use serde_json::Value;
use tokio::sync::oneshot;

async fn start_server() -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("ephemeral listener");
    let address = listener.local_addr().expect("listener address");
    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        serve(listener, router())
            .with_graceful_shutdown(async move {
                let _ = signal.await;
            })
            .await
            .expect("health server");
    });
    (format!("http://{address}"), shutdown, task)
}

async fn get_json(base: &str, path: &str) -> Value {
    reqwest::get(format!("{base}{path}"))
        .await
        .expect("request")
        .error_for_status()
        .expect("successful status")
        .json()
        .await
        .expect("JSON response")
}

#[tokio::test]
async fn health_and_readiness_are_live() {
    let (base, shutdown, task) = start_server().await;
    assert_eq!(
        get_json(&base, "/healthz").await,
        serde_json::json!({"status": "ok"})
    );
    assert_eq!(
        get_json(&base, "/readyz").await,
        serde_json::json!({"status": "ready"})
    );
    shutdown.send(()).expect("server is listening");
    task.await.expect("server task");
}

#[tokio::test]
async fn openapi_is_a_three_point_document() {
    let (base, shutdown, task) = start_server().await;
    let document = get_json(&base, "/openapi.json").await;
    assert!(document["openapi"]
        .as_str()
        .is_some_and(|version| version.starts_with('3')));
    shutdown.send(()).expect("server is listening");
    task.await.expect("server task");
}
