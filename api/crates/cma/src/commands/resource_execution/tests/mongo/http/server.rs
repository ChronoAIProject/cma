use axum::{
    body::Body, extract::State, http::StatusCode, middleware::Next, response::Response, Router,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{net::TcpListener, task::JoinHandle};

pub(super) struct Server {
    pub base: String,
    task: JoinHandle<()>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    pub async fn start(app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::serve(listener, app)
    }

    pub fn serve(listener: TcpListener, app: Router) -> Self {
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap()
        });
        Self {
            base,
            task,
            shutdown: Some(shutdown),
        }
    }

    pub async fn stop(&mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// The real handler has admitted the request. Fail its body at the HTTP transport.
pub(super) async fn lose_reply(
    State(armed): State<Arc<AtomicBool>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let create =
        request.method() == "POST" && request.uri().path() == "/api/v1/resource-revocations";
    let mut response = next.run(request).await;
    if create && response.status() == StatusCode::ACCEPTED && armed.swap(false, Ordering::SeqCst) {
        *response.body_mut() = Body::from_stream(futures_util::stream::once(async {
            Err::<axum::body::Bytes, _>(std::io::Error::other("synthetic response loss"))
        }));
    }
    response
}
