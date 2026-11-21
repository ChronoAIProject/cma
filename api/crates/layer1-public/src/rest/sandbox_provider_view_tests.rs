use super::*;
use layer2_core::profiles::PrivateProfileCreationService;

#[tokio::test]
async fn sandbox_detail_projects_pinned_providers_while_internal_profile_stays_private() {
    for access in [ModelAccess::ChronoaiManaged, ModelAccess::OwnCredentials] {
        let fixture = fixture(10);
        let mut content = sandbox_content(4);
        content.model_access = access;
        content.software_install_commands = vec!["echo instance-install".into()];
        let revision = PrivateProfileCreationService::new(
            fixture.profiles.clone(),
            bounds(),
            Arc::new(["PRIVATE_VALUE".into()]),
        )
        .create_sandbox_for_owner(
            &SubjectId::new("owner"),
            "provider-view",
            "Instance".into(),
            String::new(),
            content,
        )
        .await
        .unwrap();
        let mut sandbox = stored_sandbox("sbx_provider", "owner", SandboxStatus::Active, 4);
        sandbox.profile = SandboxProfileSnapshot::from(&revision);
        fixture.store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut profile = fixture
            .profiles
            .sandbox_profile(&revision.profile_id)
            .await
            .unwrap()
            .unwrap();
        assert!(profile.internal);
        let old_version = profile.draft_version;
        profile.draft.model_access = if access == ModelAccess::ChronoaiManaged {
            ModelAccess::OwnCredentials
        } else {
            ModelAccess::ChronoaiManaged
        };
        profile.draft_version += 1;
        fixture
            .profiles
            .save_sandbox_draft(old_version, profile)
            .await
            .unwrap();
        let profiles =
            crate::rest::profiles_tests::fixture_with_profile_store(fixture.profiles.clone());
        let profile_path = format!(
            "/api/v1/sandbox-profiles/{}/revisions/1",
            revision.profile_id
        );
        let (status, problem) = json_response(
            &profiles,
            authenticated(Method::GET, &profile_path, None, None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(problem["code"], "profile_not_found");
        let path = format!("/api/v1/sandboxes/{}", sandbox.id);
        let (status, detail) = json_response(
            &fixture.router,
            authenticated(Method::GET, &path, None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["agent_provider"], "codex");
        assert_eq!(
            detail["model_access"],
            serde_json::to_value(access).unwrap()
        );
        assert_eq!(detail["profile"]["revision"], 1);
        assert!(detail["profile"].get("content").is_none());
        assert!(detail.get("software_install_commands").is_none());
        assert!(detail.get("environment_variables").is_none());
        assert!(!detail.to_string().contains("instance-install"));
        let (status, _) = json_response(
            &fixture.router,
            request(
                Method::GET,
                &path,
                None,
                Some(("other", all_capabilities())),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
