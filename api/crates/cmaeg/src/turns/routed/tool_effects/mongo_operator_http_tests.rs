use super::{
    mongo_progress_fixture::*,
    mongo_rollback_tests::{operator, request},
    *,
};
use crate::replicas::operator_http::{self, Command};
use axum::{extract::ConnectInfo, http::StatusCode};
use std::net::SocketAddr;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_http_binds_loopback_body_and_modes_without_provider_or_delivery_io() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.nodes[owner]
                .journal()
                .transition(&mut lease, Phase::Unknown)
                .await
                .unwrap();
            h.nodes[owner].journal().release(&mut lease).await.unwrap();
            let op = operator(h);
            let req = request(h).await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let router = operator_http::router(
                op.clone(),
                h.database.keys(),
                "https://replica-fixture.invalid",
                port,
            );
            let remote = router.clone().layer(axum::middleware::from_fn(
                |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
                    request
                        .extensions_mut()
                        .insert(ConnectInfo("8.8.8.8:1000".parse::<SocketAddr>().unwrap()));
                    next.run(request).await
                },
            ));
            let remote_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let remote_port = remote_listener.local_addr().unwrap().port();
            let _remote = Task::new(tokio::spawn(async move {
                axum::serve(
                    remote_listener,
                    remote.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                .unwrap();
            }));
            let _server = Task::new(tokio::spawn(async move {
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                .unwrap();
            }));
            let command = Command {
                action: "dry_run".into(),
                request: Some(req.clone()),
                audit: None,
            };
            let bytes = serde_json::to_vec(&command).unwrap();
            let ticket = operator_http::ticket(
                &h.database.keys(),
                "https://replica-fixture.invalid",
                port,
                &bytes,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .unwrap();
            let client = reqwest::Client::new();
            let send = |bytes: Vec<u8>, ticket: String, port: u16| {
                client
                    .post(format!("http://127.0.0.1:{port}{}", operator_http::PATH))
                    .bearer_auth(ticket)
                    .body(bytes)
                    .send()
            };
            assert_eq!(
                send(bytes.clone(), ticket.clone(), remote_port)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            let apply = serde_json::to_vec(&Command {
                action: "apply".into(),
                request: Some(req.clone()),
                audit: None,
            })
            .unwrap();
            assert_eq!(
                send(apply.clone(), ticket.clone(), port)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            let dry = send(bytes, ticket, port).await.unwrap();
            assert_eq!(dry.status(), StatusCode::OK);
            let dry: serde_json::Value = dry.json().await.unwrap();
            assert_eq!(
                dry["candidate"]["binding"]["head_version"],
                req.binding.head_version
            );
            assert_eq!(
                op.inspect().await.unwrap().candidates[0]
                    .binding
                    .head_version,
                req.binding.head_version
            );
            let auth = operator_http::ticket(
                &h.database.keys(),
                "https://replica-fixture.invalid",
                port,
                &apply,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .unwrap();
            let applied = send(apply, auth, port).await.unwrap();
            assert_eq!(applied.status(), StatusCode::OK);
            let value: serde_json::Value = applied.json().await.unwrap();
            assert_eq!(
                value["rollback_safe_after"],
                dry["candidate"]["rollback_safe_after"]
            );
            assert_eq!(value["authority_key_id"], 1);
            assert_eq!(value["disposition"], "unknown_abandoned");
            assert_eq!(
                h.current.calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "operator never resolves provider authority"
            );
            assert!(
                h.rx.try_recv().is_err(),
                "operator has no delivery command path"
            );
            assert!(op.inspect().await.unwrap().candidates.is_empty());
        })
    })
    .await;
}
