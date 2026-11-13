use super::*;
fn until() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 200
}
fn handle(object: &Value) -> RuntimeHandle {
    RuntimeHandle::new(format!(
        "agent-sandbox:{}:sandbox-uid",
        object["metadata"]["name"].as_str().unwrap()
    ))
}

#[tokio::test]
async fn funding_deadline_is_uid_and_version_fenced_and_retains_home() {
    let (request, object) = object();
    let until = until();
    let mut updated = object.clone();
    updated["spec"]["shutdownTime"] = json!(super::super::funding::deadline(until).unwrap());
    updated["spec"]["shutdownPolicy"] = json!("Retain");
    let fixture = Fixture::start(vec![
        (200, object.clone()),
        (200, updated.clone()),
        (200, updated),
    ])
    .await;
    fixture
        .client
        .set_runtime_deadline(&handle(&object), &request, until)
        .await
        .unwrap();
    fixture
        .client
        .set_runtime_deadline(&handle(&object), &request, until)
        .await
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[1].0.starts_with("PATCH "));
    assert_eq!(calls[1].1["metadata"]["uid"], "sandbox-uid");
    assert_eq!(calls[1].1["metadata"]["resourceVersion"], "7");
    assert_eq!(calls[1].1["spec"]["shutdownPolicy"], "Retain");
    assert!(calls
        .iter()
        .all(|(path, _)| !path.contains("sandboxclaims")));
}
#[tokio::test]
async fn funding_expired_generation_cannot_be_extended() {
    let (request, mut object) = object();
    object["spec"]["shutdownTime"] = json!("2020-01-01T00:00:00Z");
    let fixture = Fixture::start(vec![(200, object.clone())]).await;
    assert!(fixture
        .client
        .set_runtime_deadline(&handle(&object), &request, until())
        .await
        .is_err());
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn funding_conflict_missing_runtime_and_unconfirmed_patch_fail_closed() {
    for status in [404, 409, 200] {
        let (request, object) = object();
        let replies = match status {
            404 => vec![(404, missing())],
            409 => vec![
                (200, object.clone()),
                (
                    409,
                    json!({"status":"Failure","reason":"Conflict","code":409,"message":"conflict"}),
                ),
            ],
            _ => vec![(200, object.clone()), (200, object.clone())],
        };
        let fixture = Fixture::start(replies).await;
        assert!(fixture
            .client
            .set_runtime_deadline(&handle(&object), &request, until())
            .await
            .is_err());
    }
}
#[test]
fn funding_creation_has_finite_expiry_without_changing_immutable_request_digest() {
    let request = super::super::tests::request();
    let config = super::super::tests::config();
    let unfunded = model::desired(&config, &request, "control").unwrap();
    assert!(unfunded.data["spec"].get("shutdownTime").is_none());
    let mut funded = request.clone();
    funded.funded_until = Some(until());
    let object = model::desired(&config, &funded, "control").unwrap();
    assert_eq!(object.data["spec"]["shutdownPolicy"], "Retain");
    assert_eq!(object.annotations(), unfunded.annotations());
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        serde_json::to_value(&funded).unwrap()
    );
    funded.funded_until = Some(1);
    assert!(model::desired(&config, &funded, "control").is_err());
    funded.funded_until = Some(until() + 1000);
    assert!(model::desired(&config, &funded, "control").is_err());
}

fn funded_object(mut object: Value, until: u64, version: &str) -> Value {
    object["spec"]["shutdownTime"] = json!(super::super::funding::deadline(until).unwrap());
    object["spec"]["shutdownPolicy"] = json!("Retain");
    object["metadata"]["resourceVersion"] = json!(version);
    object
}

#[tokio::test]
async fn two_and_three_callers_cannot_shorten_newer_confirmed_funding() {
    for count in [2, 3] {
        let (request, object) = object();
        let earlier = until();
        let newer = earlier + 30;
        let extended = newer + 30;
        let initial = funded_object(object.clone(), earlier - 30, "7");
        let confirmed = funded_object(object.clone(), newer, "8");
        let final_value = funded_object(object.clone(), extended, "9");
        let mut replies = vec![(200, initial), (200, confirmed.clone())];
        // Each delayed caller observes the latest native revision but carries
        // an older funding receipt. It must stop before issuing a PATCH.
        replies.extend((1..count).map(|_| (200, confirmed.clone())));
        replies.extend([
            (200, confirmed.clone()), // exact replay
            (200, confirmed),
            (200, final_value), // a subsequent valid extension
        ]);
        let fixture = Fixture::start(replies).await;
        let clients: Vec<_> = (0..count)
            .map(|_| {
                fixture
                    .client
                    .clone()
                    .with_monotonic_funding_deadlines(true)
            })
            .collect();
        let handle = handle(&object);
        clients[0]
            .set_runtime_deadline(&handle, &request, newer)
            .await
            .unwrap();
        for client in &clients[1..] {
            assert!(client
                .set_runtime_deadline(&handle, &request, earlier)
                .await
                .is_err());
        }
        clients[1]
            .set_runtime_deadline(&handle, &request, newer)
            .await
            .unwrap();
        clients[count - 1]
            .set_runtime_deadline(&handle, &request, extended)
            .await
            .unwrap();
        let calls = fixture.calls.lock().unwrap();
        let patches: Vec<_> = calls
            .iter()
            .filter(|(path, _)| path.starts_with("PATCH "))
            .collect();
        assert_eq!(calls.len(), count + 4);
        assert_eq!(patches.len(), 2);
        for ((_, patch), (until, version)) in
            patches.into_iter().zip([(newer, "7"), (extended, "8")])
        {
            assert_eq!(patch["metadata"]["resourceVersion"], version);
            assert_eq!(patch["metadata"]["uid"], "sandbox-uid");
            assert_eq!(
                patch["spec"]["shutdownTime"],
                super::super::funding::deadline(until).unwrap()
            );
            assert_eq!(patch["spec"]["shutdownPolicy"], "Retain");
        }
    }
}

#[tokio::test]
async fn default_client_preserves_existing_deadline_replacement() {
    let (request, object) = object();
    let earlier = until();
    let fixture = Fixture::start(vec![
        (200, funded_object(object.clone(), earlier + 30, "7")),
        (200, funded_object(object.clone(), earlier, "8")),
    ])
    .await;
    fixture
        .client
        .set_runtime_deadline(&handle(&object), &request, earlier)
        .await
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].0.starts_with("PATCH "));
}
