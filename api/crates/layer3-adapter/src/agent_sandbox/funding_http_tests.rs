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
