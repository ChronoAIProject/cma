use axum::{body::Body, http::Request};
use tower::ServiceExt;

use super::super::*;
use mongo_lifecycle_route_tests::{call, routes};

fn covered_request(
    store: &CreationOutcomes,
    owner: &SubjectId,
    day: i64,
    nonce: &str,
    body: &str,
) -> LookupRequest {
    let mut request = query(store, owner, Operation::AgentBootstrap, day, nonce);
    let epoch = &store.config.epoch;
    let digest = contract::wire_digest(body.as_bytes());
    let binding = contract::hash_fields(&[
        "cma-creation-key-v1",
        epoch,
        &day.to_string(),
        nonce,
        owner.as_str(),
        store.deployment(),
        "POST",
        Operation::AgentBootstrap.path(),
        &digest,
    ]);
    request.idempotency_key = format!("cma1.{epoch}.{day}.{nonce}.{binding}");
    request.fingerprint.digest = digest;
    request
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn covered_validation_refusal_is_atomic_and_replays_with_no_allocation() {
    let (mut store, mut rotated, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let (off, _, _off_workers) = routes(&store).await;
    store.config.validation_settlement = true;
    let (on, _, _on_workers) = routes(&store).await;
    rotated.config.validation_settlement = true;
    rotated.config.epoch = "f".repeat(32);
    let (after_rotation, _, _rotated_workers) = routes(&rotated).await;
    for (nonce, body, problem_code, field) in [
        (
            "a",
            r#"{"name":"Fixture Desktop","desktop":true}"#,
            "invalid_request",
            None,
        ),
        (
            "b",
            r#"{"name":"Fixture template","desktop":true,"template":{"id":"research","version":1,"fields":{}}}"#,
            "task_template_field_required",
            Some("topic"),
        ),
    ] {
        let request = covered_request(&store, &owner, today, &nonce.repeat(32), body);
        let submit = |body: &str| {
            Request::post(Operation::AgentBootstrap.path())
                .header("content-type", "application/json")
                .header("Idempotency-Key", &request.idempotency_key)
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let baseline = call(&off, submit(body), StatusCode::UNPROCESSABLE_ENTITY).await;
        assert_eq!(baseline["code"], problem_code);
        assert_eq!(baseline["field"].as_str(), field);
        let exact = identity(&store, &owner, &request);
        assert_eq!(
            store.lookup(&exact, &request).await.unwrap(),
            Outcome::Unknown {
                reason: UnknownReason::CoverageUnavailable,
            }
        );
        // Discard the initial response and read the committed result from a
        // separate Mongo client before replaying the original wire request.
        let first = on.clone().oneshot(submit(body)).await.unwrap();
        assert_eq!(first.status(), StatusCode::UNPROCESSABLE_ENTITY);
        drop(first);
        let proof = rotated.lookup(&exact, &request).await.unwrap();
        assert!(matches!(
            proof,
            Outcome::Refused {
                code: RefusalCode::CreationFailed,
                operation_id: None,
                ..
            }
        ));
        assert_eq!(
            call(&on, submit(body), StatusCode::UNPROCESSABLE_ENTITY).await,
            baseline
        );
        // A wire change preserves the semantic error but cannot reuse the key.
        call(&on, submit(&format!("{body} ")), StatusCode::CONFLICT).await;
        let before = store.records.count_documents(doc! {}).await.unwrap();
        assert_eq!(
            call(
                &after_rotation,
                submit(body),
                StatusCode::UNPROCESSABLE_ENTITY
            )
            .await,
            baseline
        );
        call(
            &after_rotation,
            submit(&format!("{body} ")),
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(rotated.lookup(&exact, &request).await.unwrap(), proof);
        assert!(matches!(
            rotated.begin(&exact).await.unwrap(),
            Begin::Closed
        ));
        assert_eq!(
            store.records.count_documents(doc! {}).await.unwrap(),
            before
        );
    }
    let unadmitted_body = r#"{"name":"Unadmitted Fixture Desktop","desktop":true}"#;
    let unadmitted = covered_request(&store, &owner, today, &"3".repeat(32), unadmitted_body);
    let before = store.records.count_documents(doc! {}).await.unwrap();
    call(
        &after_rotation,
        Request::post(Operation::AgentBootstrap.path())
            .header("content-type", "application/json")
            .header("Idempotency-Key", &unadmitted.idempotency_key)
            .body(Body::from(unadmitted_body))
            .unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        store.records.count_documents(doc! {}).await.unwrap(),
        before
    );
    assert_no_allocations(&store).await;
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn linked_pending_and_expired_proofs_cannot_become_validation_refusals() {
    let (mut store, reader, today) = fixture().await;
    store.config.validation_settlement = true;
    let owner = SubjectId::new("owner");
    let (app, _, _worker_slots) = routes(&store).await;
    let body = r#"{"name":"Fixture Desktop","desktop":true}"#;
    let request = covered_request(&store, &owner, today, &"c".repeat(32), body);
    let exact = identity(&store, &owner, &request);
    let Begin::Acquired(fence) = store.begin(&exact).await.unwrap() else {
        panic!("fixture should acquire the original pending proof");
    };
    let submit = || {
        Request::post(Operation::AgentBootstrap.path())
            .header("content-type", "application/json")
            .header("Idempotency-Key", &request.idempotency_key)
            .body(Body::from(body))
            .unwrap()
    };
    call(&app, submit(), StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        reader.lookup(&exact, &request).await.unwrap(),
        Outcome::InProgress { operation_id: None }
    );
    let mut session = store.database.client().start_session().await.unwrap();
    transaction::start_transaction(&mut session).await.unwrap();
    assert!(publication::link_operation(
        &store.database,
        owner.as_str(),
        &fence,
        &"d".repeat(32),
        &mut session,
    )
    .await
    .unwrap());
    assert!(matches!(
        transaction::commit_transaction(&mut session).await,
        transaction::CommitOutcome::Committed
    ));
    call(&app, submit(), StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        store
            .records
            .count_documents(doc! {"_id": &fence.record_id, "state.kind":"pending", "operation_id":"d".repeat(32)})
            .await
            .unwrap(),
        1
    );

    let expired = covered_request(&store, &owner, today, &"e".repeat(32), body);
    let expired_identity = identity(&store, &owner, &expired);
    let Begin::Acquired(expiring) = store.begin(&expired_identity).await.unwrap() else {
        panic!("fixture should acquire the expiring proof");
    };
    store
        .records
        .update_one(
            doc! {"_id":expiring.record_id},
            doc! {"$set":{"lease_until":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    store.settle_expired_unlinked().await.unwrap();
    let proof = reader.lookup(&expired_identity, &expired).await.unwrap();
    assert!(matches!(
        proof,
        Outcome::Refused {
            code: RefusalCode::CreationExpired,
            ..
        }
    ));
    assert_eq!(
        store
            .refuse_validation(
                &expired_identity,
                ValidationRefusal::DesktopInitialMessageRequired
            )
            .await,
        Err(StatusCode::CONFLICT)
    );
    assert_eq!(
        reader.lookup(&expired_identity, &expired).await.unwrap(),
        proof
    );
    assert_no_allocations(&store).await;
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn validation_detail_write_failure_aborts_the_refusal_and_pending_proof() {
    let (mut store, reader, today) = fixture().await;
    store.config.validation_settlement = true;
    let owner = SubjectId::new("owner");
    let body = r#"{"name":"Fixture Desktop","desktop":true}"#;
    let request = covered_request(&store, &owner, today, &"f".repeat(32), body);
    let exact = identity(&store, &owner, &request);
    let detail_id = format!("validation:{}", record_id(&exact));
    store
        .records
        .insert_one(doc! {"_id": &detail_id, "owner": owner.as_str() })
        .await
        .unwrap();

    let refusal = ValidationRefusal::DesktopInitialMessageRequired;
    assert_eq!(
        store.refuse_validation(&exact, refusal).await,
        Err(StatusCode::SERVICE_UNAVAILABLE)
    );
    assert_eq!(
        reader.lookup(&exact, &request).await.unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::CoverageUnavailable
        }
    );
    assert_eq!(
        store
            .records
            .delete_one(doc! { "_id": detail_id })
            .await
            .unwrap()
            .deleted_count,
        1
    );
    store.refuse_validation(&exact, refusal).await.unwrap();
    assert!(matches!(
        reader.lookup(&exact, &request).await.unwrap(),
        Outcome::Refused {
            code: RefusalCode::CreationFailed,
            operation_id: None,
            ..
        }
    ));
    assert_no_allocations(&store).await;
    store.database.drop().await.unwrap();
}

async fn assert_no_allocations(store: &CreationOutcomes) {
    for collection in [
        crate::store::agent_lifecycle_operations::COLLECTION,
        "sandboxes",
        "workspaces",
        "agents",
    ] {
        assert_eq!(
            store
                .database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "{collection}",
        );
    }
}
