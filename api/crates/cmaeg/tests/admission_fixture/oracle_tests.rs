use super::*;
use axum::{routing::get, Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::seal::{seal_connection, seal_interaction};

#[tokio::test]
async fn fifo_head_uses_connection_namespace_and_sealing() {
    let mut key = [0u8; 32];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut key).unwrap();
    let keys = Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode(key))).unwrap());
    let id = uuid::Uuid::new_v4().to_string();
    let head = json!({"kind":"cmaeg_partition_v1","queue":[],"authority":null});
    let item = json!({"kind":"cmaeg_partition_item_v1","done":true});
    let head_wire =
        STANDARD.encode(seal_connection(&keys, &id, &serde_json::to_vec(&head).unwrap()).unwrap());
    let item_wire =
        STANDARD.encode(seal_interaction(&keys, &id, &serde_json::to_vec(&item).unwrap()).unwrap());
    let router = Router::new()
        .route(
            &format!("/api/v1/gateway/connection-records/{id}"),
            get(move || {
                let blob = head_wire.clone();
                async move { Json(json!({"blob":blob,"version":13})) }
            }),
        )
        .route(
            &format!("/api/v1/gateway/interaction-records/{id}"),
            get(move || {
                let blob = item_wire.clone();
                async move { Json(json!({"blob":blob,"version":2})) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let records = RecordClient::new(
        &format!("http://{address}"),
        &super::super::bootstrap::random_secret().unwrap(),
    )
    .unwrap();
    let receipts = std::env::temp_dir().join(format!("eg-oracle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&receipts).unwrap();
    let oracle = Oracle {
        plans: PlanStore::new(records.clone(), keys.clone()),
        records: records.interactions(),
        keys,
        client: mongodb::Client::with_uri_str("mongodb://127.0.0.1:1")
            .await
            .unwrap(),
        receipts: receipts.clone(),
    };
    let observed_head = oracle.retained(&id, false).await.unwrap().unwrap();
    assert_eq!(observed_head.0, head);
    assert_eq!(observed_head.1, 13);
    let observed_item = oracle.retained(&id, true).await.unwrap().unwrap();
    assert_eq!(observed_item.0, item);
    assert_eq!(observed_item.1, 2);
    oracle.shutdown().await;
    stop.send(()).unwrap();
    server.await.unwrap();
    std::fs::remove_dir_all(receipts).unwrap();
}
