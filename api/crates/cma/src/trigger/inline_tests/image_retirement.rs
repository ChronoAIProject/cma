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
    let retired = authoring(Arc::new(crate::agent_lifecycle::RetiredImageResolver));

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
async fn editing_runtime_settings_cannot_admit_a_new_retired_image_profile() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        profiles.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(crate::agent_lifecycle::RetiredImageResolver),
    );
    let owner = SubjectId::new("trigger-owner");
    let existing = target();
    let mut edit = existing.clone();
    edit.inline.as_mut().unwrap().agent.model = Some("gpt-6".into());
    assert_eq!(
        materializer
            .materialize_draft_update(&owner, &edit, &existing, Some("owner-fixture"))
            .await
            .unwrap_err(),
        TriggerReferenceError::ImageSelectionUnsupported {
            field: "target.inline.image",
        },
    );
    assert!(profiles
        .list_agent_profiles(None, false, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
