use super::{
    creation, management,
    mutation_fixture::{owner, wait, Fixture},
};
use cmaeg_core::ids::RecordId;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn adding_a_producer_to_incomplete_setup_names_the_precondition() {
    let fixture = Fixture::new(false, false).await;
    let first = fixture.state().await;
    let second = fixture.state().await;
    let mut body = fixture.body();
    body["partition"]["key"] = "fixed".into();
    let request = serde_json::from_value(body.clone()).unwrap();
    fixture.peer.pause.store(true, Ordering::SeqCst);
    let mut creation =
        tokio::spawn(
            async move { management::create(&first, &owner("alice", false), request).await },
        );
    tokio::select! {
        () = wait(&fixture.peer.started) => {},
        result = &mut creation => panic!("creation ended before the provider barrier: {result:?}"),
    }
    let creator = owner("alice", false);
    let record = RecordId::new(body["record_id"].as_str().unwrap()).unwrap();
    let pending = creation::load_record(&second, &creator, &record)
        .await
        .unwrap();
    assert_eq!(pending.record.version, 1);
    let mut update = fixture.update(&body, 1, "With another producer");
    let producer: super::SourceRequest = serde_json::from_value(serde_json::json!({
            "type":"custom", "auth":{"type":"nyxid_key","key_hashes":[format!("sha256:{}", "a".repeat(64))]}
    }))
    .unwrap();
    update.sources.push(producer.clone());
    let refused = management::update(&second, &creator, &pending.channel_id, update).await;
    let routing_refused = super::routing_update::update(
        &second, &creator, &pending.channel_id,
        serde_json::from_value(serde_json::json!({
            "expected_version":1,
            "routing":{"version":1,"default_consumer":{"name":"default"},"default_targets":["default"]}
        })).unwrap(),
    ).await;
    let calls = fixture.peer.calls.lock().unwrap().clone();
    fixture.peer.resume.notify_one();
    let created = creation.await.unwrap().unwrap();
    assert_eq!(
        calls,
        ["PUT"],
        "the refused edit must send no provider request"
    );
    let error = match refused {
        Ok(_) => panic!("incomplete setup admitted a producer update"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("channel setup is incomplete"),
        "equal versions must not obscure the setup precondition: {error}"
    );
    assert!(matches!(
        routing_refused,
        Err(management::ManagementError::CreationPending)
    ));
    let mut ready_update = fixture.update(&body, created.version, "Ready");
    ready_update.sources.push(producer);
    let updated = management::update(&second, &creator, &created.channel_id, ready_update)
        .await
        .unwrap();
    assert_eq!(updated.definition.sources.len(), 2);
    assert_eq!(updated.record.version, created.version + 1);
    let stale = management::update(
        &second,
        &creator,
        &created.channel_id,
        fixture.update(&body, created.version, "Stale"),
    )
    .await;
    assert!(matches!(stale, Err(management::ManagementError::Conflict {
        expected, actual: Some(actual)
    }) if expected == created.version && actual == updated.record.version));
    fixture.cleanup().await;
}
