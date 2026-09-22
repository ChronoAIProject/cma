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
#[tokio::test]
async fn profile_resolution_pins_published_defaults_and_conceals_private_agent_revisions() {
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
    let pinned = service
        .resolve(&SubjectId::new("owner"), &defaults.agent_profile)
        .await
        .unwrap();
    assert_eq!(
        service
            .resolve(&SubjectId::new("foreign"), &defaults.agent_profile)
            .await,
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
            .resolve(&SubjectId::new("owner"), &defaults.agent_profile)
            .await
            .unwrap(),
        pinned
    );
    store
        .retire_agent_profile(&defaults.agent_profile.id, &SubjectId::new("owner"), now)
        .await
        .unwrap();
    assert_eq!(
        service
            .resolve(&SubjectId::new("owner"), &defaults.agent_profile)
            .await,
        Err(BotError::NotFound)
    );
}
