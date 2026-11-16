use super::*;

#[tokio::test]
async fn queued_owner_routing_preserves_source_revision_and_exact_edit_or_cancel() {
    for suffix in ["queued-input", "queued-cancellation"] {
        let f = fixture(false).await;
        let path = PATH.replace("controls", suffix);
        let mut body = json!({"source_command_id":SOURCE,"expected_command_revision":1});
        if suffix == "queued-input" {
            body["text"] = json!("Replacement input");
        }
        let (status, first) = call(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            Some("queued-owner-intent"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let work_after_first = f
            .store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items;
        let calls = f.route.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (target, key) = match &calls[0] {
            OwnerCommandMutation::QueuedInput(value) if suffix == "queued-input" => {
                assert_eq!(value.text, "Replacement input");
                (&value.target, &value.idempotency_key)
            }
            OwnerCommandMutation::QueuedCancellation(value) if suffix == "queued-cancellation" => {
                (&value.target, &value.idempotency_key)
            }
            _ => panic!("wrong queued operation"),
        };
        assert_eq!(target.source_command_id.as_str(), SOURCE);
        assert_eq!(target.response_id.as_str(), "resp_control_source");
        assert_eq!(target.expected_command_revision, 1);
        assert_eq!(key, "queued-owner-intent");
        let (status, replay) = call(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            Some("queued-owner-intent"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(replay, first);
        body["expected_command_revision"] = json!(2);
        let (status, conflict) = call(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            Some("queued-owner-intent"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
        assert_eq!(conflict["error"]["code"], "agent_command_key_conflict");
        assert_eq!(f.route.calls.lock().unwrap().len(), 3);
        assert_eq!(
            f.store()
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items,
            work_after_first
        );
    }
}

#[tokio::test]
async fn interruption_owner_routing_preserves_one_atomic_pair_on_exact_replay() {
    let f = fixture(true).await;
    let mut body = intent(&f.original, json!({"type":"cancel"})).await;
    body.as_object_mut().unwrap().remove("command");
    body["text"] = json!("Replacement after interruption");
    let path = PATH.replace("controls", "interrupt");
    let (status, first) = call(
        &f.router,
        Method::POST,
        &path,
        "owner-a",
        Some("paired-intent"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    let (status, replay) = call(
        &f.router,
        Method::POST,
        &path,
        "owner-a",
        Some("paired-intent"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
    assert_eq!(first, replay);
    assert_ne!(
        first["control"]["receipt"]["command_id"],
        first["response"]["receipt"]["command_id"]
    );
    assert_eq!(
        f.store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        3
    );
    let calls = f.route.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    for call in calls.iter() {
        let OwnerCommandMutation::InterruptingMessage(value) = call else {
            panic!("split interruption")
        };
        assert_eq!(value.idempotency_key, "paired-intent");
        assert_eq!(value.text, "Replacement after interruption");
    }
}
