use super::*;
use crate::{
    profiles::{store::*, *},
    *,
};
use serde_json::json;
use std::sync::Arc;

struct Settings(PlatformSettings);
#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(StoredPlatformSettings {
            changes: Default::default(),
            revision: 1,
            settings: self.0.clone(),
        })
    }
    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        unreachable!()
    }
}
struct Activity;
#[async_trait::async_trait]
impl ActivityStore for Activity {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(vec![], None))
    }
}
async fn fixture() -> (
    Arc<InMemoryProfileStore>,
    PlatformDefaults,
    Arc<PublishedBotProfiles>,
) {
    let store = Arc::new(InMemoryProfileStore::default());
    let defaults = PlatformDefaults {
        agent_profile: AgentProfileRef::new(AgentProfileId("agp_bot".into()), 1),
        sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_bot".into()), 1),
        workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp_bot".into()), 1),
    };
    let now = now_rfc3339();
    let metadata = json!({"owner":"owner","name":"Bot environment","description":"","visibility":"public","draft_version":1,"created_at":now,"updated_at":now});
    let mut agent = metadata.clone();
    agent["visibility"] = "private".into();
    agent["id"] = json!(defaults.agent_profile.id);
    agent["draft"] = json!({"model":null,"thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":"published"});
    store
        .create_agent_profile(serde_json::from_value(agent).unwrap())
        .await
        .unwrap();
    store
        .publish_agent_profile(&defaults.agent_profile.id, 1, now.clone())
        .await
        .unwrap();
    let mut workspace = metadata.clone();
    workspace["id"] = json!(defaults.workspace_profile.id);
    workspace["draft"] = json!({"location":{"type":"directory","name":"project"},"name":"project","setup_commands":[],"agent_profiles":[]});
    store
        .create_workspace_profile(serde_json::from_value(workspace).unwrap())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&defaults.workspace_profile.id, 1, now.clone())
        .await
        .unwrap();
    let mut sandbox = metadata;
    sandbox["id"] = json!(defaults.sandbox_profile.id);
    sandbox["draft"] = json!({"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":AgentProvider::default(),"model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]});
    store
        .create_sandbox_profile(serde_json::from_value(sandbox).unwrap())
        .await
        .unwrap();
    store
        .publish_sandbox_profile(
            &defaults.sandbox_profile.id,
            1,
            now.clone(),
            ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            &[] as &[String; 0],
        )
        .await
        .unwrap();
    let settings = PlatformSettings {
        defaults: defaults.clone(),
        ..Default::default()
    };
    let service = PublishedBotProfiles::new(
        store.clone(),
        Arc::new(PlatformSettingsService::new(
            Arc::new(Settings(settings)),
            ActivityService::new(Arc::new(Activity)),
        )),
        "configured-model".into(),
    );
    (store, defaults, service)
}

#[tokio::test]
async fn profile_resolution_pins_published_defaults_and_conceals_private_agent_revisions() {
    let (store, defaults, service) = fixture().await;
    let now = now_rfc3339();
    let mut request = super::tests::input();
    request.profile = defaults.agent_profile.clone();
    let pinned = service
        .resolve(&SubjectId::new("owner"), &request)
        .await
        .unwrap();
    assert_eq!(
        service.resolve(&SubjectId::new("foreign"), &request).await,
        Err(BotError::NotFound)
    );
    assert_eq!(pinned.resolved_model, "configured-model");
    assert_eq!(pinned.agent.content.system_prompt, "published");
    let mut draft = store
        .agent_profile(&defaults.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    draft.draft_version += 1;
    draft.draft.system_prompt = "new draft".into();
    store.save_agent_draft(1, draft).await.unwrap();
    assert_eq!(
        service
            .resolve(&SubjectId::new("owner"), &request)
            .await
            .unwrap(),
        pinned
    );
    store
        .retire_agent_profile(&defaults.agent_profile.id, &SubjectId::new("owner"), now)
        .await
        .unwrap();
    assert_eq!(
        service.resolve(&SubjectId::new("owner"), &request).await,
        Err(BotError::NotFound)
    );
}

#[tokio::test]
async fn selected_environments_preserve_published_content_and_require_current_ownership() {
    let (store, defaults, service) = fixture().await;
    let owner = SubjectId::new("owner");
    let now = now_rfc3339();
    let mut sandbox = store
        .sandbox_profile(&defaults.sandbox_profile.id)
        .await
        .unwrap()
        .unwrap();
    sandbox.id = SandboxProfileId("sbp_custom".into());
    sandbox.draft.memory_mib = 8192;
    sandbox.draft.cpu_millis = 4000;
    sandbox.draft.software_install_commands = vec!["mkdir -p /home/cma/tools".into()];
    let sandbox_ref = SandboxProfileRef::new(sandbox.id.clone(), 1);
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(
            &sandbox.id,
            1,
            now.clone(),
            ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            &[] as &[String; 0],
        )
        .await
        .unwrap();
    let mut workspace = store
        .workspace_profile(&defaults.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    workspace.id = WorkspaceProfileId("wkp_custom".into());
    workspace.draft.location = WorkspaceLocation::GitRepository {
        url: "https://github.com/example/research".into(),
        reference: None,
    };
    workspace.draft.setup_commands = vec!["mkdir -p reports".into()];
    let workspace_ref = WorkspaceProfileRef::new(workspace.id.clone(), 1);
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, now.clone())
        .await
        .unwrap();
    let mut request = super::tests::input();
    request.profile = defaults.agent_profile.clone();
    request.sandbox_profile = Some(sandbox_ref.clone());
    let first = service.resolve(&owner, &request).await.unwrap();
    assert_eq!(first.workspace.profile_id, defaults.workspace_profile.id);
    request.workspace_profile = Some(workspace_ref.clone());
    let pinned = service.resolve(&owner, &request).await.unwrap();
    assert_eq!(pinned.sandbox.content, sandbox.draft);
    assert_eq!(pinned.workspace.content, workspace.draft);
    sandbox.draft_version += 1;
    sandbox.draft.memory_mib = 1024;
    store.save_sandbox_draft(1, sandbox.clone()).await.unwrap();
    workspace.draft_version += 1;
    workspace.draft.setup_commands.clear();
    store
        .save_workspace_draft(1, workspace.clone())
        .await
        .unwrap();
    assert_eq!(service.resolve(&owner, &request).await.unwrap(), pinned);
    request.workspace_profile.as_mut().unwrap().revision = 2;
    assert_eq!(
        service.resolve(&owner, &request).await,
        Err(BotError::NotFound)
    );
    request.workspace_profile = Some(workspace_ref);
    // Even publicly readable foreign environment revisions cannot admit their
    // install/setup commands under another person's Bot authority.
    let mut foreign_sandbox = sandbox.clone();
    foreign_sandbox.id = SandboxProfileId("sbp_foreign".into());
    foreign_sandbox.owner = SubjectId::new("foreign");
    foreign_sandbox.draft_version = 1;
    store
        .create_sandbox_profile(foreign_sandbox.clone())
        .await
        .unwrap();
    store
        .publish_sandbox_profile(
            &foreign_sandbox.id,
            1,
            now.clone(),
            ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            &[] as &[String; 0],
        )
        .await
        .unwrap();
    request.sandbox_profile = Some(SandboxProfileRef::new(foreign_sandbox.id, 1));
    assert_eq!(
        service.resolve(&owner, &request).await,
        Err(BotError::NotFound)
    );
    request.sandbox_profile = Some(sandbox_ref.clone());
    let mut foreign_workspace = workspace.clone();
    foreign_workspace.id = WorkspaceProfileId("wkp_foreign".into());
    foreign_workspace.owner = SubjectId::new("foreign");
    foreign_workspace.draft_version = 1;
    store
        .create_workspace_profile(foreign_workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&foreign_workspace.id, 1, now.clone())
        .await
        .unwrap();
    request.workspace_profile = Some(WorkspaceProfileRef::new(foreign_workspace.id, 1));
    assert_eq!(
        service.resolve(&owner, &request).await,
        Err(BotError::NotFound)
    );
    request.workspace_profile = None;
    store
        .retire_sandbox_profile(&sandbox_ref.id, &owner, now)
        .await
        .unwrap();
    assert_eq!(
        service.resolve(&owner, &request).await,
        Err(BotError::NotFound)
    );
}

#[tokio::test]
async fn optional_environment_refs_preserve_legacy_fingerprints_and_fence_pinned_records() {
    let legacy = super::tests::input();
    let value = serde_json::to_value(&legacy).unwrap();
    assert!(value.get("sandbox_profile").is_none());
    assert!(value.get("workspace_profile").is_none());
    assert_eq!(
        serde_json::from_value::<CreateBot>(value.clone()).unwrap(),
        legacy
    );
    assert_eq!(
        content_digest(&value).unwrap(),
        content_digest(&legacy).unwrap()
    );
    let service = BotService::new(
        Arc::new(InMemoryBotStore::default()),
        Arc::new(super::tests::Profiles),
        "cma".into(),
    );
    let caller = super::tests::caller("owner");
    let original = service
        .create(&caller, "retained", legacy.clone())
        .await
        .unwrap();
    for sandbox in [false, true] {
        let mut selected = legacy.clone();
        if sandbox {
            selected.sandbox_profile = Some(SandboxProfileRef::new(
                original.profiles.sandbox.profile_id.clone(),
                1,
            ));
        } else {
            selected.workspace_profile = Some(WorkspaceProfileRef::new(
                original.profiles.workspace.profile_id.clone(),
                1,
            ));
        }
        assert_eq!(
            service.create(&caller, "retained", selected.clone()).await,
            Err(BotError::Conflict)
        );
        let mut bot = service
            .create(
                &caller,
                if sandbox {
                    "selected-sandbox"
                } else {
                    "selected-workspace"
                },
                selected,
            )
            .await
            .unwrap();
        if sandbox {
            bot.profiles.sandbox.revision += 1;
        } else {
            bot.profiles.workspace.revision += 1;
        }
        assert!(bot.validate().is_err());
    }
}
