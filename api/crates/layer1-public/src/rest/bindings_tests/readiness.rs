use super::*;
use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};

pub(super) async fn create_backing_resources(
    store: &InMemorySandboxStore,
    binding: &ProviderBinding,
) {
    store
        .create_sandbox(super::super::agents_tests::sandbox(
            binding.sandbox_id.as_str(),
            binding.owner.as_str(),
        ))
        .await
        .unwrap();
    store
        .create_workspace(super::super::agents_tests::workspace(
            binding.workspace_id.as_str(),
            binding.sandbox_id.as_str(),
            binding.owner.as_str(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn stale_ready_profile_routes_refuse_read_and_ensure_but_allow_retirement() {
    for affinity in ["conversation", "source_user"] {
        let (app, store) = app();
        let path = "/api/v1/cmaeg/bindings/stale-binding";
        let mut input = body();
        input["profile"]["environment"]["session_affinity"] = affinity.into();
        input["profile_digest"] =
            format!("sha256:{}", content_digest(&input["profile"]).unwrap()).into();
        let response = app
            .clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut binding = store.binding("stale-binding").await.unwrap().unwrap();
        let revision = binding.revision;
        binding.state = BindingState::Ready;
        binding.work = BindingWork::Idle;
        binding
            .advance_revision(layer2_core::now_rfc3339())
            .unwrap();
        store
            .save_binding(Some(revision), binding.clone())
            .await
            .unwrap();
        let conversation = format!("{path}/conversations/test_conversation");
        for (method, target, body) in [
            ("GET", path, None),
            ("PUT", path, Some(input)),
            ("PUT", conversation.as_str(), None),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, target, body, Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let problem = json_body(response).await;
            assert_eq!(problem["code"], "binding_resources_unavailable");
            assert!(problem["detail"]
                .as_str()
                .unwrap()
                .contains("Reconnect the channel"));
            assert_eq!(
                store.binding("stale-binding").await.unwrap(),
                Some(binding.clone())
            );
            assert!(store
                .binding_conversations("stale-binding", binding.generation, None, 100)
                .await
                .unwrap()
                .is_empty());
        }
        for target in [conversation.as_str(), path] {
            let response = app
                .clone()
                .oneshot(request("DELETE", target, None, Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
        assert_eq!(
            store.binding("stale-binding").await.unwrap().unwrap().state,
            BindingState::Retired
        );
    }
}
