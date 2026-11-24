use super::*;
use layer2_core::trigger_domain::{
    InMemoryLaunchStore, InMemoryTriggerStore, TriggerContent, TriggerService, TriggerServiceError,
};
use layer2_core::{
    Authorization, Caller, Capability, DirectoryQuery, Page, PublicIdentity, RegisteredUser,
    StoreError, UserDirectory, UserDirectoryStore,
};
use serde_json::json;

struct EmptyDirectory;
struct LegacyVersionlessImage;

#[async_trait]
impl UserDirectoryStore for EmptyDirectory {
    async fn observe(&self, _: &PublicIdentity) -> Result<(), StoreError> {
        Ok(())
    }

    async fn user(&self, _: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        Ok(None)
    }

    async fn list(&self, _: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

#[async_trait]
impl ImageResolver for LegacyVersionlessImage {
    async fn resolve(
        &self,
        credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, layer1_public::rest::AgentLifecycleProvisioningError> {
        assert_eq!(credential, Some("trigger-caller"));
        assert_eq!(image.id, "private-codex");
        assert!(image.version.is_none());
        Ok(format!("registry.example/codex@sha256:{}", "a".repeat(64)))
    }
}

#[tokio::test]
async fn title_edit_accepts_the_original_versionless_public_selection() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let definitions = Arc::new(InMemoryTriggerStore::new(profiles.clone()));
    let launches = Arc::new(InMemoryLaunchStore::new(definitions.clone()));
    let directory = UserDirectory::new(Arc::new(EmptyDirectory));
    let authoring = |resolver: Arc<dyn ImageResolver>| {
        TriggerService::new_with_inline_materializer(
            definitions.clone(),
            launches.clone(),
            directory.clone(),
            InlineTriggerProfileMaterializer::new(
                profiles.clone(),
                bounds(),
                layer3_adapter::SandboxCapacity::default(),
                Arc::new([] as [String; 0]),
                resolver,
            ),
        )
    };
    let publisher = Caller::new(
        SubjectId::new("trigger-owner"),
        Authorization::new([Capability::PublishTriggers]),
    );
    let mut original = json!({
        "presentation":{"title":"Original title","description":"","button_label":"open_in_cma","theme":"auto"},
        "target":target(),
        "reuse":{"sandbox":"always_new","workspace":"always_new","agent":"always_new"},
        "first_message":"",
        "consent":{"always_review":true}
    });
    original["target"]["inline"]["image"] = json!({"id": "private-codex"});
    let stored = authoring(Arc::new(LegacyVersionlessImage))
        .create_with_credential(
            &publisher,
            serde_json::from_value(original.clone()).unwrap(),
            Some("trigger-caller"),
        )
        .await
        .unwrap();
    let resolved = stored
        .draft
        .target
        .inline
        .as_ref()
        .unwrap()
        .image
        .as_ref()
        .unwrap();
    assert!(resolved.version.is_none());
    assert!(resolved.reference.is_some());
    let retired = authoring(
        crate::agent_lifecycle::ImageManagerResolver::new("http://127.0.0.1:1".into(), true)
            .unwrap(),
    );

    original["presentation"]["title"] = json!("Edited title");
    let edited = retired
        .update_draft(
            &publisher,
            &stored.id,
            stored.draft_version,
            serde_json::from_value::<TriggerContent>(original.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edited.draft.presentation.title, "Edited title");
    assert_eq!(edited.draft.target, stored.draft.target);

    original["target"]["inline"]["agent"]["model"] = json!("gpt-6");
    let refusal = retired
        .update_draft(
            &publisher,
            &stored.id,
            edited.draft_version,
            serde_json::from_value(original).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refusal,
        TriggerServiceError::Reference(TriggerReferenceError::ImageSelectionUnsupported {
            field: "target.inline.image",
        }),
    );
    assert_eq!(retired.read(&publisher, &stored.id).await.unwrap(), edited);
}

#[tokio::test]
async fn retirement_rechecks_current_image_authority_for_new_trigger_profiles() {
    use axum::{
        http::{header, HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
        Json, Router,
    };
    use serde_json::Value;

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
