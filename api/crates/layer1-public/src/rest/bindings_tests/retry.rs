use super::*;
use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};

#[tokio::test]
async fn failed_binding_put_requires_recovered_original_hierarchy_then_replays_ready() {
    let (app, store) = app_with_tools(Some("https://gateway.example/mcp"));
    let path = "/api/v1/cmaeg/bindings/recovered-binding";
    let input = body();
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut binding = store.binding("recovered-binding").await.unwrap().unwrap();
    let revision = binding.revision;
    binding.state = BindingState::Failed {
        reason: "sandbox provisioning failed".into(),
    };
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let mut sandbox =
        super::super::agents_tests::sandbox(binding.sandbox_id.as_str(), binding.owner.as_str());
    sandbox.status = SandboxStatus::Active;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    sandbox.credential.source = binding.credential_source();
    sandbox.credential.key_generation = 1;
    sandbox.credential.delivered = true;
    sandbox.credential.pending = None;
    sandbox.credential.current = Some(SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: format!("provided-{}", binding.key_sha256),
        provider_state_version: 1,
        issued_at: layer2_core::now_rfc3339(),
        expires_at: String::new(),
        grant_fingerprint: content_digest(&sandbox.credential.effective_reaches()).unwrap(),
    });
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut workspace = super::super::agents_tests::workspace(
        binding.workspace_id.as_str(),
        binding.sandbox_id.as_str(),
        binding.owner.as_str(),
    );
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, layer2_core::now_rfc3339());
    store.create_workspace(workspace.clone()).await.unwrap();
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let revision = workspace.revision;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, layer2_core::now_rfc3339());
    workspace.revision += 1;
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    for case in 0..3 {
        let mut changed = input.clone();
        match case {
            0 => changed["agent_key"] = json!("nyx_different"),
            1 => {
                changed["profile"]["metadata"]["owner"]["subject"] = json!("other-owner");
                changed["profile_digest"] = json!(format!(
                    "sha256:{}",
                    content_digest(&changed["profile"]).unwrap()
                ));
            }
            _ => changed["tools"]["native_mcp_url"] = json!("https://other.example/mcp"),
        }
        let response = app
            .clone()
            .oneshot(request("PUT", path, Some(changed), Some(authority())))
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        assert_eq!(
            store.binding(&binding.binding_id).await.unwrap(),
            Some(binding.clone())
        );
    }
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ready = json_body(response).await;
    assert_eq!(ready["state"], "ready");
    let stored = store.binding(&binding.binding_id).await.unwrap().unwrap();
    assert_eq!(stored.generation, binding.generation);
    assert_eq!(stored.sandbox_id, binding.sandbox_id);
    assert_eq!(stored.workspace_id, binding.workspace_id);
    assert_eq!(stored.revision, binding.revision + 1);
    assert_eq!(
        json_body(
            app.clone()
                .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
                .await
                .unwrap()
        )
        .await,
        ready
    );
    assert_eq!(
        store.get_sandbox(&binding.sandbox_id).await.unwrap(),
        Some(sandbox)
    );
    assert_eq!(
        store.get_workspace(&binding.workspace_id).await.unwrap(),
        Some(workspace)
    );
    assert!(store
        .binding_conversations(&binding.binding_id, binding.generation, None, 100)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        app.clone()
            .oneshot(request("DELETE", path, None, Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.oneshot(request("PUT", path, Some(input), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        store
            .binding(&binding.binding_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        BindingState::Retired
    );
}
