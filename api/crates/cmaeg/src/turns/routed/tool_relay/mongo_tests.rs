use super::*;
use crate::turns::routed::{delivery_tests::command, replica_test_fixture::*};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn nonowner_relay_returns_only_the_owners_durable_receipt() {
    let mut h = Harness::new().await;
    let (owner, _lease) = h.accepted().await;
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", 20021 + owner))
        .await
        .unwrap();
    let router = h.tools[owner].clone().replica_relay_router();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let other = 1 - owner;
    let tools = h.tools[other].clone();
    let channel = h.current.channel.lock().unwrap().clone();
    let event = h.event();
    let (result, ()) = tokio::join!(
        tools.reply_with_idempotency(
            caller(),
            &channel,
            &event,
            message(),
            false,
            Some("relayed")
        ),
        async {
            let reply = tokio::time::timeout(Duration::from_secs(5), command(&mut h.rx))
                .await
                .unwrap();
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        }
    );
    let first = result.unwrap().unwrap();
    let replay = tools
        .reply_with_idempotency(
            caller(),
            &channel,
            &event,
            message(),
            false,
            Some("relayed"),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        storage::bytes(&first).unwrap(),
        storage::bytes(&replay).unwrap()
    );
    assert!(h.rx.try_recv().is_err());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn receiver_rejects_stale_expected_fence_without_adopting_current_owner() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    let fence = h.nodes[owner]
        .journal()
        .partition_owner(&h.plan, h.target())
        .await
        .unwrap()
        .unwrap()
        .fence;
    h.nodes[owner].journal().release(&mut lease).await.unwrap();
    let (next, _next_lease) = h.acquire().await;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let envelope = codec::seal(
        &h.database.keys(),
        Direction::Request,
        &uuid::Uuid::new_v4().to_string(),
        CONTEXT,
        now + 24,
        now,
        &Request {
            caller_key_hash: caller().key_hash,
            event: EventRef::seal(&h.event(), &h.database.keys()).unwrap(),
            message: message(),
            idempotency_key: Some("stale".into()),
            expected: fence,
        },
    )
    .unwrap();
    let bytes = storage::bytes(&envelope).unwrap();
    let request_hash = storage::hash(&bytes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = h.tools[next].clone().replica_relay_router();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{address}{PATH}"))
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.bytes().await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    let response: Envelope = serde_json::from_slice(&bytes).unwrap();
    let outcome: Outcome = codec::open(
        &h.database.keys(),
        Direction::Response,
        &request_hash,
        now,
        &response,
    )
    .unwrap();
    assert!(matches!(outcome, Outcome::Refused(Refusal::LostLease)));
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}
