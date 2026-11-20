use super::*;

#[tokio::test]
async fn visitor_direct_creation_is_forbidden_before_any_effect() {
    let fixture = fixture(2);
    let (profile_id, _) = publish_sandbox_profile(
        &fixture.profiles,
        "sbp_visitor_boundary",
        "publisher",
        ProfileVisibility::Public,
        vec![],
    )
    .await;
    for _ in 0..2 {
        let (status, problem) = json_response(
            &fixture.router,
            request(
                Method::POST,
                "/api/v1/sandboxes",
                Some(json!({
                    "sandbox_profile": { "id": profile_id, "revision": 1 },
                    "name": "visitor-box"
                })),
                Some((
                    "visitor",
                    Authorization::new([
                        Capability::LaunchTriggers,
                        Capability::UseOwnSandboxes,
                        Capability::DeleteOwnSandbox,
                    ]),
                )),
                Some("visitor-direct-create"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(problem["code"], "forbidden");
        let sandboxes = fixture
            .store
            .list_sandboxes(&SubjectId::new("visitor"), &Default::default())
            .await
            .unwrap();
        assert_eq!(sandboxes.items.len(), 0);
        assert_eq!(fixture.operations.records.lock().unwrap().len(), 0);
    }
}
