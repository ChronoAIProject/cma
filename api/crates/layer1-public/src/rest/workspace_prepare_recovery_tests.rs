use super::*;
const PATH: &str = "/api/v1/workspaces/wks_recovery/recover-prepare";
pub(in crate::rest) async fn recovery_fixture() -> Fixture {
    let f = fixture(3);
    let mut parent = stored_sandbox("sbx_recovery", "owner", SandboxStatus::Active, 4);
    parent
        .work
        .schedule(SandboxWorkStep::WaitForCredentialRotation, NOW.into());
    let mut workspace = stored_workspace(&parent, "wks_recovery");
    workspace.name = "project".into();
    workspace.path = "/home/cma/project".into();
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    workspace.ready_at = None;
    workspace.resolved_commit = None;
    workspace.next_worktree_number = 1;
    workspace.status = WorkspaceStatus::Failed;
    workspace.failed = Some(layer2_core::sandbox::TypedFailure {
        class: "prepare_failed".into(),
        detail: "terminal".into(),
    });
    workspace.work.execution = Some(Default::default());
    f.store.create_sandbox(parent).await.unwrap();
    f.store.create_workspace(workspace).await.unwrap();
    f
}
#[tokio::test]
async fn workspace_prepare_recovery_api_enforces_auth_required_key_and_replay() {
    let f = recovery_fixture().await;
    for (subject, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(("owner", Authorization::none())),
            StatusCode::FORBIDDEN,
        ),
        (Some(("foreign", all_capabilities())), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            json_response(
                &f.router,
                request(Method::POST, PATH, None, subject, Some("auth"))
            )
            .await
            .0,
            status
        );
    }
    assert_eq!(
        json_response(&f.router, authenticated(Method::POST, PATH, None, None))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let first = json_response(
        &f.router,
        authenticated(Method::POST, PATH, None, Some("recover")),
    )
    .await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(first.1["id"], "wks_recovery");
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, None, Some("recover"))
        )
        .await,
        first
    );
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, None, Some("new"))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn workspace_prepare_recovery_api_admin_retains_real_actor_and_owner() {
    let f = recovery_fixture().await;
    assert_eq!(
        json_response(
            &f.router,
            request(
                Method::POST,
                PATH,
                None,
                Some((
                    "operator",
                    Authorization::new([Capability::ControlAnySandbox])
                )),
                Some("admin")
            )
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let workspace = f
        .store
        .get_workspace(&WorkspaceId("wks_recovery".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workspace.owner.as_str(), "owner");
    assert_eq!(
        workspace.work.prepare_recovery.unwrap().attempts[0]
            .requested_by
            .as_str(),
        "operator"
    );
}
