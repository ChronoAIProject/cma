use super::*;
use layer2_core::{
    profiles::{
        AgentProfileStore, InMemoryProfileStore, ProfileCapacityBounds, SandboxProfileStore,
        WorkspaceProfileStore,
    },
    trigger_domain::TriggerInlineTargetMaterializer,
};

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    }
}

fn target() -> TriggerTarget {
    serde_json::from_value(serde_json::json!({
        "inline": {
            "image": {
                "id": "private-codex",
                "version": "v3",
                "reference": "registry.example/codex@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            },
            "agent": {
                "model": "gpt-5-codex",
                "reasoning_effort": "high",
                "permission_mode": "guarded",
                "skills": [{"source": "ornn", "id": "repo-search"}],
                "system_prompt": "Keep changes small.",
                "services": ["api-github"]
            },
            "workspace": {
                "repository": {
                    "url": "https://github.com/chronoai/project",
                    "reference": "main"
                }
            }
        }
    }))
    .expect("inline target should decode")
}

#[tokio::test]
async fn materialization_is_deterministic_and_links_private_revisions() {
    let store = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        store.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(AuthorizedImageResolver),
    );
    let owner = SubjectId::new("trigger-owner");
    let source = target();

    let first = materializer
        .materialize_with_credential(&owner, &source, Some("trigger-caller"))
        .await
        .unwrap();
    let second = materializer
        .materialize_with_credential(&owner, &source, Some("trigger-caller"))
        .await
        .unwrap();
    assert_eq!(
        first, second,
        "same inline content must reuse its snapshots"
    );

    let agent = store
        .agent_profile(&first.agent_profile.id)
        .await
        .unwrap()
        .expect("generated agent profile");
    assert_eq!(agent.owner, owner);
    assert_eq!(
        agent.visibility,
        layer2_core::profiles::ProfileVisibility::Private
    );

    let agent_revision = store
        .agent_profile_revision(&first.agent_profile.id, first.agent_profile.revision)
        .await
        .unwrap()
        .expect("generated agent revision");
    let workspace_revision = store
        .workspace_profile_revision(
            &first.workspace_profile.id,
            first.workspace_profile.revision,
        )
        .await
        .unwrap()
        .expect("generated workspace revision");
    let sandbox_revision = store
        .sandbox_profile_revision(&first.sandbox_profile.id, first.sandbox_profile.revision)
        .await
        .unwrap()
        .expect("generated sandbox revision");

    assert_eq!(
        workspace_revision.content.agent_profiles,
        vec![AgentProfileRef::new(
            first.agent_profile.id.clone(),
            first.agent_profile.revision
        ),]
    );
    assert_eq!(
        sandbox_revision.content.workspace_profiles,
        vec![WorkspaceProfileRef::new(
            first.workspace_profile.id.clone(),
            first.workspace_profile.revision
        ),]
    );
    assert_eq!(
        sandbox_revision.content.runtime_image.as_deref(),
        Some("registry.example/codex@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(sandbox_revision.content.max_workspaces, 16);
    assert_eq!(sandbox_revision.content.max_agents_per_workspace, 16);
    assert_eq!(agent_revision.content.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(
        (
            sandbox_revision.content.cpu_millis,
            sandbox_revision.content.memory_mib
        ),
        (1000, 1024)
    );
}

#[tokio::test]
async fn changed_capacity_materializes_new_snapshots_and_retains_existing_target() {
    let store = Arc::new(InMemoryProfileStore::default());
    let owner = SubjectId::new("trigger-owner");
    let build = |capacity| {
        InlineTriggerProfileMaterializer::new(
            store.clone(),
            bounds(),
            capacity,
            Arc::new([] as [String; 0]),
            Arc::new(AuthorizedImageResolver),
        )
    };
    let old = build(layer3_adapter::SandboxCapacity::default())
        .materialize_with_credential(&owner, &target(), Some("trigger-caller"))
        .await
        .unwrap();
    let changed = build(layer3_adapter::SandboxCapacity {
        cpu_millis: 2000,
        memory_mib: 4096,
    });
    let new = changed
        .materialize_with_credential(&owner, &target(), Some("trigger-caller"))
        .await
        .unwrap();
    assert_ne!(new.sandbox_profile.id, old.sandbox_profile.id);
    assert_eq!(
        changed
            .materialize_with_credential(&owner, &target(), Some("trigger-caller"))
            .await
            .unwrap(),
        new
    );
    for (target, expected) in [(old, (1000, 1024)), (new, (2000, 4096))] {
        let retained = store
            .sandbox_profile_revision(&target.sandbox_profile.id, target.sandbox_profile.revision)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (retained.content.cpu_millis, retained.content.memory_mib),
            expected
        );
    }
}

#[tokio::test]
async fn changed_inline_content_and_owner_get_distinct_snapshots() {
    let store = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        store.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(AuthorizedImageResolver),
    );
    let owner = SubjectId::new("trigger-owner");
    let mut changed = target();
    changed.inline.as_mut().expect("inline target").agent.model = Some("gpt-5-codex-next".into());

    let first = materializer
        .materialize_with_credential(&owner, &target(), Some("trigger-caller"))
        .await
        .unwrap();
    let second = materializer
        .materialize_with_credential(&owner, &changed, Some("trigger-caller"))
        .await
        .unwrap();
    assert_ne!(first.agent_profile.id, second.agent_profile.id);
    assert_ne!(first.workspace_profile.id, second.workspace_profile.id);
    assert_ne!(first.sandbox_profile.id, second.sandbox_profile.id);

    let other_owner = SubjectId::new("other-owner");
    let other = materializer
        .materialize_with_credential(&other_owner, &target(), Some("trigger-caller"))
        .await
        .unwrap();
    assert_ne!(first.agent_profile.id, other.agent_profile.id);
    let other_agent = store
        .agent_profile(&other.agent_profile.id)
        .await
        .unwrap()
        .expect("other owner's generated agent");
    assert_eq!(other_agent.owner, other_owner);
}

#[tokio::test]
async fn invalid_inline_payload_is_rejected_before_profile_creation() {
    let store = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        store.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(AuthorizedImageResolver),
    );
    let mut invalid = target();
    invalid.inline.as_mut().unwrap().agent.system_prompt = "x".repeat(32_769);

    let error = materializer
        .materialize_with_credential(
            &SubjectId::new("trigger-owner"),
            &invalid,
            Some("trigger-caller"),
        )
        .await
        .expect_err("oversized Agent instructions should fail");
    assert_eq!(error.field(), "target.inline.agent.system_prompt");
    assert!(store
        .list_agent_profiles(None, false, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

struct AuthorizedImageResolver;

#[async_trait]
impl ImageResolver for AuthorizedImageResolver {
    async fn resolve(
        &self,
        credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, layer1_public::rest::AgentLifecycleProvisioningError> {
        if credential.is_none() {
            return Err(layer1_public::rest::AgentLifecycleProvisioningError::SignInRequired);
        }
        if credential != Some("trigger-caller") {
            return Err(layer1_public::rest::AgentLifecycleProvisioningError::ImageAccessDenied);
        }
        assert_eq!(image.id, "private-codex");
        assert_eq!(image.version.as_deref(), Some("v3"));
        Ok(format!("registry.example/codex@sha256:{}", "a".repeat(64)))
    }
}

#[tokio::test]
async fn image_authority_is_required_before_creating_private_profiles() {
    let store = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        store.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(AuthorizedImageResolver),
    );
    for credential in [None, Some("unauthorized-caller")] {
        let error = materializer
            .materialize_with_credential(&SubjectId::new("trigger-owner"), &target(), credential)
            .await
            .expect_err("a supplied digest cannot bypass Image Manager authority");
        assert_eq!(error.field(), "target.inline.image");
        assert!(matches!(
            (credential, error),
            (None, TriggerReferenceError::ImageSignInRequired { .. })
                | (
                    Some("unauthorized-caller"),
                    TriggerReferenceError::ImageAccessDenied { .. }
                )
        ));
    }
    assert!(store
        .list_agent_profiles(None, false, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .list_workspace_profiles(None, false, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .list_sandbox_profiles(None, false, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn image_manager_reference_replaces_supplied_or_missing_digest() {
    let store = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        store.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(AuthorizedImageResolver),
    );
    let mut supplied = target();
    for reference in [
        None,
        Some(format!("untrusted.example/other@sha256:{}", "b".repeat(64))),
    ] {
        supplied
            .inline
            .as_mut()
            .unwrap()
            .image
            .as_mut()
            .unwrap()
            .reference = reference;
        let result = materializer
            .materialize_with_credential(
                &SubjectId::new("trigger-owner"),
                &supplied,
                Some("trigger-caller"),
            )
            .await
            .unwrap();
        let image = result.inline.as_ref().unwrap().image.as_ref().unwrap();
        let expected = format!("registry.example/codex@sha256:{}", "a".repeat(64));
        assert_eq!(image.reference.as_deref(), Some(expected.as_str()));
        let sandbox = store
            .sandbox_profile_revision(&result.sandbox_profile.id, result.sandbox_profile.revision)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sandbox.content.runtime_image, Some(expected));
    }
}

#[tokio::test]
async fn system_default_image_does_not_require_image_manager_authority() {
    let store = Arc::new(InMemoryProfileStore::default());
    let materializer = InlineTriggerProfileMaterializer::new(
        store.clone(),
        bounds(),
        layer3_adapter::SandboxCapacity::default(),
        Arc::new([] as [String; 0]),
        Arc::new(AuthorizedImageResolver),
    );
    let mut source = target();
    source.inline.as_mut().unwrap().image = None;
    let result = materializer
        .materialize(&SubjectId::new("trigger-owner"), &source)
        .await
        .unwrap();
    let sandbox = store
        .sandbox_profile_revision(&result.sandbox_profile.id, result.sandbox_profile.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox.content.runtime_image, None);
}

#[path = "inline_tests/image_retirement.rs"]
mod image_retirement;
