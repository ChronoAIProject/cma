//! Retire persistent connections for new requests without cancelling their work.
use std::{
    future::Future,
    io,
    net::SocketAddr,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Request},
    http::{header, HeaderValue, StatusCode, Version},
    middleware::{from_fn, Next},
    serve::Listener,
    Extension, Router,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use tokio::{
    net::TcpListener,
    sync::{watch, Notify},
    task::JoinSet,
};
use tower::{Layer, ServiceExt};

const AGE_ENV: &str = "CHRONO_HTTP_CONNECTION_MAX_AGE_SECONDS";

pub(crate) fn max_age_from_env() -> Result<Option<Duration>, String> {
    let value = std::env::var(AGE_ENV).map_err(|error| match error {
        std::env::VarError::NotPresent => None,
        std::env::VarError::NotUnicode(_) => Some(format!("{AGE_ENV} must be UTF-8")),
    });
    match value {
        Ok(value) => parse_max_age(&value),
        Err(None) => Ok(None),
        Err(Some(error)) => Err(error),
    }
}

fn parse_max_age(value: &str) -> Result<Option<Duration>, String> {
    match value.parse::<u64>() {
        Ok(0) => Ok(None),
        Ok(seconds @ 30..=3600) => Ok(Some(Duration::from_secs(seconds))),
        _ => Err(format!("{AGE_ENV} must be 0 (disabled) or 30..=3600")),
    }
}

pub(crate) async fn serve(
    mut listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
    max_age: Option<Duration>,
) -> io::Result<()> {
    let Some(max_age) = max_age else {
        return axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await;
    };
    tokio::pin!(shutdown);
    let (stop, _) = watch::channel(false);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break,
            Some(result) = connections.join_next() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "HTTP connection task failed");
                }
            }
            (socket, peer) = Listener::accept(&mut listener) => {
                let expires = Instant::now() + max_age;
                let h2_seen = std::sync::Arc::new(Notify::new());
                let protocol = h2_seen.clone();
                // Retire H1 through its response, not by closing an idle socket
                // concurrently with a client's next non-replayable request.
                let retire_h1 = from_fn(move |request: Request, next: Next| {
                    let h2 = request.version() == Version::HTTP_2;
                    if h2 { protocol.notify_one(); }
                    async move {
                        let mut response = next.run(request).await;
                        if !h2 && response.status() != StatusCode::SWITCHING_PROTOCOLS && Instant::now() >= expires {
                            response.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("close"));
                        }
                        response
                    }
                });
                // Use one outer layer, as Axum's make-service does, rather than
                // rebuilding every route per connection. Preserve real TCP peer.
                let service = TowerToHyperService::new(retire_h1.layer(
                    Extension(ConnectInfo(peer)).layer(app.clone()),
                ).map_request(|request: axum::http::Request<hyper::body::Incoming>| request.map(Body::new)));
                let mut stop = stop.subscribe();
                connections.spawn(async move {
                    let mut builder = Builder::new(TokioExecutor::new());
                    // Preserve Axum's HTTP/2 extended CONNECT/WebSocket support.
                    builder.http2().enable_connect_protocol();
                    let connection = builder.serve_connection_with_upgrades(TokioIo::new(socket), service);
                    tokio::pin!(connection);
                    tokio::select! {
                        result = &mut connection => {
                            if let Err(error) = result {
                                tracing::trace!(%error, "HTTP connection ended");
                            }
                            return;
                        }
                        _ = async {
                            tokio::time::sleep(max_age).await;
                            // notify_one retains the permit from an earlier H2
                            // request; a late first request also starts drain.
                            h2_seen.notified().await;
                        } => {}
                        _ = stop.changed() => {}
                    }
                    // Age sends H2 GOAWAY; process shutdown drains either protocol.
                    // Existing bodies/upgrades retain their lifetime. This is
                    // admission age, never a request or stream timeout.
                    connection.as_mut().graceful_shutdown();
                    if let Err(error) = connection.await {
                        tracing::trace!(%error, "HTTP connection drain ended");
                    }
                });
            }
        }
    }
    drop(listener);
    let _ = stop.send(true);
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::warn!(%error, "HTTP connection task failed during drain");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "http_server/tests.rs"]
mod tests;
