//! Serve HTTP with a bounded connection age.
//!
//! A long-lived client connection pins every request it carries to one
//! replica, because the Service balances connections rather than requests.
//! Retiring each connection after a bounded age lets new requests rebalance.
//! Retirement is graceful: HTTP/1 closes after its current response, HTTP/2
//! sends GOAWAY and lets open streams finish, and an upgraded connection has
//! already left this server.

use std::{future::Future, time::Duration};

use axum::{extract::connect_info::ConnectInfo, Router};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
    service::TowerToHyperService,
};
use tokio::{net::TcpListener, sync::watch};
use tower::Layer as _;

/// Accept until `shutdown` resolves, then retire every connection gracefully
/// and return once all of them have finished, like `axum::serve`.
pub(crate) async fn serve(
    listener: TcpListener,
    app: Router,
    max_age: Duration,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let (stop, stopped) = watch::channel(false);
    let (closed, open) = watch::channel(());
    tokio::pin!(shutdown);
    let mut accepted = 0_u64;
    loop {
        let (stream, peer) = tokio::select! {
            result = listener.accept() => match result {
                Ok(pair) => pair,
                Err(error) if connection_error(&error) => continue,
                Err(error) => {
                    // Matches axum::serve: back off instead of spinning on EMFILE.
                    tracing::error!(%error, "HTTP accept failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        accepted = accepted.wrapping_add(1);
        let service = axum::Extension(ConnectInfo(peer)).layer(app.clone());
        let age = jittered(max_age, accepted);
        let mut stopped = stopped.clone();
        let open = open.clone();
        tokio::spawn(async move {
            let builder = auto::Builder::new(TokioExecutor::new());
            let connection = builder.serve_connection_with_upgrades(
                TokioIo::new(stream),
                TowerToHyperService::new(service),
            );
            tokio::pin!(connection);
            let expiry = tokio::time::sleep(age);
            tokio::pin!(expiry);
            let mut retiring = false;
            loop {
                tokio::select! {
                    result = connection.as_mut() => {
                        if let Err(error) = result {
                            tracing::debug!(%peer, error = %error, "HTTP connection ended with an error");
                        }
                        break;
                    }
                    () = &mut expiry, if !retiring => {
                        retiring = true;
                        connection.as_mut().graceful_shutdown();
                    }
                    _ = stopped.changed(), if !retiring => {
                        retiring = true;
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
            drop(open);
        });
    }
    drop(listener);
    let _ = stop.send(true);
    drop(open);
    closed.closed().await;
    Ok(())
}

/// Spread retirements over up to ten percent so connections accepted together
/// do not all reconnect in the same instant.
fn jittered(age: Duration, accepted: u64) -> Duration {
    age + age.mul_f64((accepted % 11) as f64 / 100.0)
}

fn connection_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
#[path = "http_serving_tests.rs"]
mod tests;
