//! Loopback public ingress shared by modeled and actual receiver contracts.
use axum::{http::HeaderMap, Router};
use layer2_core::AgentId;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub(super) const ORIGIN: &str = "http://desktop.example.test";

pub(super) struct Ingress {
    address: std::net::SocketAddr,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Ingress {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl Ingress {
    pub(super) async fn start(app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            address,
            task: Some(task),
        }
    }

    pub(super) fn websocket(
        &self,
        id: &AgentId,
        origin: &str,
        headers: &HeaderMap,
    ) -> axum::http::Request<()> {
        let mut request = format!("ws://{}/api/v1/agents/{id}/desktop/ws", self.address)
            .into_client_request()
            .unwrap();
        request.headers_mut().extend(headers.clone());
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        request
    }

    pub(super) async fn discovery(&self, id: &AgentId, headers: &HeaderMap) -> reqwest::Response {
        self.discovery_result(id, headers).await.unwrap()
    }

    pub(super) async fn discovery_result(
        &self,
        id: &AgentId,
        headers: &HeaderMap,
    ) -> reqwest::Result<reqwest::Response> {
        reqwest::Client::new()
            .get(format!(
                "http://{}/api/v1/agents/{id}/desktop",
                self.address
            ))
            .headers(headers.clone())
            .send()
            .await
    }

    pub(super) async fn stop(mut self) {
        let task = self.task.take().unwrap();
        task.abort();
        let stopped = task.await;
        assert!(stopped.is_ok() || stopped.unwrap_err().is_cancelled());
    }
}
