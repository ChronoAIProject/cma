use super::*;

#[tokio::test]
async fn source_user_transport_lifecycle_never_reserves_or_retires_an_agent() {
    let (app, store) = app();
    let path = "/api/v1/cmaeg/bindings/binding-users";
    let mut input = body();
    input["profile"]["environment"]["session_affinity"] = json!("source_user");
    input["profile_digest"] = json!(format!(
        "sha256:{}",
        content_digest(&input["profile"]).unwrap()
    ));
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(input), Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut binding = store.binding("binding-users").await.unwrap().unwrap();
    assert_eq!(
        binding.profile.session_affinity,
        SessionAffinity::SourceUser
    );
    let revision = binding.revision;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    let generation = binding.generation;
    readiness::create_backing_resources(&store, &binding).await;
    store.save_binding(Some(revision), binding).await.unwrap();
    for transport in ["chat_one", "chat_two", "cma_user_untrusted_alias"] {
        let target = format!("{path}/conversations/{transport}");
        let response = app
            .clone()
            .oneshot(request("PUT", &target, None, Some(authority())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await,
            json!({"conversation_id":transport,"state":"ready"})
        );
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(request("DELETE", &target, None, Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
    }
    assert!(store
        .binding_conversations("binding-users", generation, None, 100)
        .await
        .unwrap()
        .is_empty());
    for method in ["PUT", "DELETE"] {
        let response = app
            .clone()
            .oneshot(request(
                method,
                &format!("{path}/conversations/bad"),
                None,
                Some(authority()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app
            .clone()
            .oneshot(request(
                method,
                &format!("{path}/conversations/chat_one"),
                None,
                Some(Authorization::none()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
