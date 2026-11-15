use super::*;

pub(super) struct Fixture {
    pub address: std::net::SocketAddr,
    supervisor: Arc<Supervisor>,
    server: tokio::task::JoinHandle<()>,
    display_server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    pub async fn start(config: crate::config::RuntimeConfig, token: String) -> Self {
        let supervisor =
            Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
        let display = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let display_url = format!("ws://{}/display", display.local_addr().unwrap());
        let display_server = tokio::spawn(async move {
            let mut streams = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = display.accept().await.unwrap();
                streams.spawn(async move {
                    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(message)) = socket.next().await {
                        if message.is_close() {
                            break;
                        }
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                });
                while streams.try_join_next().is_some() {}
            }
        });
        let auth = crate::control_auth::ControlAuth::from_token(token.clone()).unwrap();
        let receiver = supervisor.desktop_ownership.clone();
        let app = Router::new()
            .route(
                "/runtime/desktop",
                get(
                    move |ws: WebSocketUpgrade, headers: axum::http::HeaderMap| {
                        upgrade(
                            ws,
                            receiver.clone(),
                            "display-test".into(),
                            display_url.clone(),
                            headers,
                        )
                    },
                ),
            )
            .route(
                "/runtime/desktop/ownership/challenge",
                post(crate::link::desktop_control::challenge),
            )
            .route(
                "/runtime/desktop/ownership/confirm",
                post(crate::link::desktop_control::confirm),
            )
            .route(
                "/runtime/desktop/ownership/revoke",
                post(crate::link::desktop_control::revoke),
            )
            .with_state(supervisor.clone())
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::control_auth::authenticate,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            address,
            supervisor,
            server,
            display_server,
        }
    }

    pub async fn stop(mut self) {
        self.supervisor.desktop_ownership.stop_fixture().await;
        self.server.abort();
        self.display_server.abort();
        let _ = (&mut self.server).await;
        let _ = (&mut self.display_server).await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        self.display_server.abort();
    }
}
