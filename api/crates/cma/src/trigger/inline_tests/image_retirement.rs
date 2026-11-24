use super::*;

#[tokio::test]
async fn retirement_rechecks_current_image_authority_for_new_trigger_profiles() {
    use axum::{
        http::{header, HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
        Json, Router,
    };
    use serde_json::{json, Value};

    let catalog = Router::new().route(
        "/v1/images/private-codex/resolve",
        post(|headers: HeaderMap, Json(input): Json<Value>| async move {
            assert_eq!(input, json!({"version": "v3"}));
            let hex = match headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
            {
                Some("Bearer allowed-fixture") => "a",
                Some("Bearer changed-digest-fixture") => "b",
                _ => return StatusCode::FORBIDDEN.into_response(),
            };
            let digest = format!("sha256:{}", hex.repeat(64));
            Json(json!({
                "id": "v3",
                "reference": format!("registry.example/codex@{digest}"),
                "digest": digest,
            }))
            .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let resolver = crate::agent_lifecycle::ImageManagerResolver::new(
        format!("http://{}", listener.local_addr().unwrap()),
        true,
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, catalog).await.unwrap() });
    let profiles = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        profiles.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        resolver,
    );
    let owner = SubjectId::new("trigger-owner");
    let existing = target();
    let mut edit = target();
    edit.inline.as_mut().unwrap().agent.model = Some("gpt-6".into());
    assert_eq!(
        materializer.materialize(&owner, &edit).await.unwrap_err(),
        TriggerReferenceError::ImageSelectionUnsupported {
            field: "target.inline.image"
        },
    );
    assert_eq!(
        materializer
            .materialize_draft_update(&owner, &existing, &existing, Some("revoked-fixture"))
            .await
            .unwrap(),
        existing,
    );
    for (credential, expected) in [
        (
            "revoked-fixture",
            TriggerReferenceError::ImageAccessDenied {
                field: "target.inline.image",
            },
        ),
        (
            "changed-digest-fixture",
            TriggerReferenceError::ImageSelectionUnsupported {
                field: "target.inline.image",
            },
        ),
    ] {
        assert_eq!(
            materializer
                .materialize_draft_update(&owner, &edit, &existing, Some(credential))
                .await
                .unwrap_err(),
            expected,
        );
    }
    assert!(profiles
        .list_agent_profiles(None, false, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());

    let saved = materializer
        .materialize_draft_update(&owner, &edit, &existing, Some("allowed-fixture"))
        .await
        .unwrap();
    let pinned = existing.inline.unwrap().image.unwrap();
    assert_eq!(saved.inline.unwrap().image.as_ref(), Some(&pinned));
    let sandbox = profiles
        .sandbox_profile_revision(&saved.sandbox_profile.id, saved.sandbox_profile.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox.content.runtime_image, pinned.reference);
    let agent = profiles
        .agent_profile_revision(&saved.agent_profile.id, saved.agent_profile.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.content.model.as_deref(), Some("gpt-6"));

    edit.inline
        .as_mut()
        .unwrap()
        .image
        .as_mut()
        .unwrap()
        .version = Some("v4".into());
    assert_eq!(
        materializer
            .materialize_draft_update(&owner, &edit, &target(), None)
            .await
            .unwrap_err(),
        TriggerReferenceError::ImageSelectionUnsupported {
            field: "target.inline.image"
        },
    );
    server.abort();
}
