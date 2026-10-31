use super::*;

#[tokio::test]
async fn inspection_reads_existing_reservations_without_creating_or_activating_agents() {
    let (app, store) = app();
    let path = "/api/v1/cmaeg/bindings/binding-inspection";
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(body()), Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut binding = store.binding("binding-inspection").await.unwrap().unwrap();
    let revision = binding.revision;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    create_backing_resources(&store, &binding).await;
    store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    let path = format!("{path}/conversations/inspection_conversation");
    let missing = app
        .clone()
        .oneshot(request("GET", &path, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let created = app
        .clone()
        .oneshot(request("PUT", &path, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json_body(created).await;
    let response = app
        .clone()
        .oneshot(request("GET", &path, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let view = json_body(response).await;
    assert_eq!(view["agent_id"], created["agent_id"]);
    assert_eq!(view["state"], "provisioning");
    assert_eq!(view["generation"], 1);
    assert!(view["agent_status"].is_null());
    assert_eq!(
        store.binding("binding-inspection").await.unwrap(),
        Some(binding)
    );
    for (authorization, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(Authorization::none()), StatusCode::FORBIDDEN),
    ] {
        let response = app
            .clone()
            .oneshot(request("GET", &path, None, authorization))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}
