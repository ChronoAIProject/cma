use super::*;
use crate::{
    bots::*, profiles::*, AgentProfileId, Authorization, Caller, Capability, SandboxProfileId,
    WorkspaceProfileId,
};
use std::sync::Arc;
struct Resolver;
#[async_trait::async_trait]
impl BotProfileResolver for Resolver {
    async fn resolve(&self, _: &SubjectId, request: &CreateBot) -> Result<BotProfiles, BotError> {
        let agent:AgentProfileContent=serde_json::from_value(serde_json::json!({"model":"example","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":"Retained instructions"})).unwrap();
        let workspace:WorkspaceProfileContent=serde_json::from_value(serde_json::json!({"location":{"type":"directory","name":"project"},"name":"project","setup_commands":[],"agent_profiles":[]})).unwrap();
        let sandbox:SandboxProfileContent=serde_json::from_value(serde_json::json!({"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":AgentProvider::default(),"model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]})).unwrap();
        Ok(BotProfiles {
            event_worker_profile: None,
            resolved_model: "example".into(),
            agent: AgentProfileRevision {
                profile_id: request.profile.id.clone(),
                revision: request.profile.revision,
                digest: content_digest(&agent).unwrap(),
                content: agent,
                published_at: "2026-09-23T00:00:00Z".into(),
            },
            workspace: WorkspaceProfileRevision {
                profile_id: request.workspace_profile.as_ref().unwrap().id.clone(),
                revision: 1,
                digest: content_digest(&workspace).unwrap(),
                content: workspace,
                published_at: "2026-09-23T00:00:00Z".into(),
            },
            sandbox: SandboxProfileRevision {
                profile_id: request.sandbox_profile.as_ref().unwrap().id.clone(),
                revision: 1,
                digest: content_digest(&sandbox).unwrap(),
                content: sandbox,
                published_at: "2026-09-23T00:00:00Z".into(),
            },
        })
    }
}
fn caller(name: &str) -> Caller {
    Caller::new(
        SubjectId::new(name),
        Authorization::new([Capability::ManageOwnProfiles, Capability::ManageOwnBots]),
    )
}
fn input() -> EventWorkerProfileInput {
    EventWorkerProfileInput {
        name: "Worker".into(),
        description: "Example configuration".into(),
        agent_profile: AgentProfileRef::new(AgentProfileId("agp_example".into()), 1),
        workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp_example".into()), 1),
        sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_example".into()), 1),
    }
}
fn service() -> Arc<EventWorkerProfileService> {
    EventWorkerProfileService::new(
        Arc::new(InMemoryEventWorkerProfileStore::default()),
        Arc::new(Resolver),
        "registered".into(),
    )
}
#[tokio::test]
async fn create_replay_retains_revision_one_across_edits_and_owner_boundaries() {
    let service = service();
    let owner = caller("owner");
    let input = input();
    let (a, b) = tokio::join!(
        service.create(&owner, "key", input.clone()),
        service.create(&owner, "key", input.clone())
    );
    let first = a.unwrap();
    assert_eq!(first, b.unwrap());
    let mut changed = input.clone();
    changed.name = "Edited".into();
    let next = service
        .update(&owner, &first.id, 1, changed.clone())
        .await
        .unwrap();
    assert_eq!(next.revision, 2);
    assert_eq!(service.create(&owner, "key", input).await.unwrap(), first);
    assert_eq!(
        service.update(&owner, &first.id, 1, changed).await.unwrap(),
        next
    );
    assert_eq!(
        service.get(&caller("other"), &first.id, None).await,
        Err(BotError::NotFound)
    );
    assert_eq!(
        service.retire(&caller("other"), &first.id, 2).await,
        Err(BotError::NotFound)
    );
    let mut conflict = first.source.clone();
    conflict.name = "conflict".into();
    assert_eq!(
        service.update(&owner, &first.id, 1, conflict).await,
        Err(BotError::Conflict)
    );
}
#[tokio::test]
async fn bots_pin_full_worker_configuration_and_retirement_does_not_rewrite_it() {
    let service = service();
    let owner = caller("owner");
    let row = service.create(&owner, "key", input()).await.unwrap();
    let request = CreateBot {
        name: "Bot".into(),
        external_bot_id: "external".into(),
        connection_ref: None,
        profile: row.source.agent_profile.clone(),
        workspace_profile: Some(row.source.workspace_profile.clone()),
        sandbox_profile: Some(row.source.sandbox_profile.clone()),
        event_worker_profile: Some(row.pin().reference),
        admission: BotAdmission::All,
        reply: BotReply {
            auto_final: true,
            ack_after_ms: 2000,
        },
    };
    let bots = BotService::new(
        Arc::new(InMemoryBotStore::default()),
        service.clone(),
        "registered".into(),
    );
    let bot = bots
        .create(&owner, "create-bot", request.clone())
        .await
        .unwrap();
    bot.validate().unwrap();
    assert_eq!(
        bot.profiles.event_worker_profile.as_ref().unwrap().snapshot,
        row.snapshot
    );
    service.retire(&owner, &row.id, 1).await.unwrap();
    assert!(service.list(&owner, None, 50).await.unwrap().is_empty());
    assert_eq!(service.get(&owner, &row.id, Some(1)).await.unwrap(), row);
    assert_eq!(
        bots.create(&owner, "create-bot", request.clone())
            .await
            .unwrap(),
        bot
    );
    assert_eq!(
        bots.create(&owner, "new-bot", request).await,
        Err(BotError::NotFound)
    );
}
#[tokio::test]
async fn profile_authority_and_snapshot_integrity_are_not_inferred_from_json() {
    let service = service();
    let owner = caller("owner");
    let row = service.create(&owner, "key", input()).await.unwrap();
    let denied = Caller::new(
        owner.subject.clone(),
        Authorization::new([Capability::ManageOwnBots]),
    );
    assert_eq!(
        service.list(&denied, None, 10).await,
        Err(BotError::Forbidden)
    );
    let mut corrupt = row;
    corrupt.snapshot["permission"]["action_mode"] = serde_json::json!("autonomous");
    corrupt.digest = format!("sha256:{}", content_digest(&corrupt.snapshot).unwrap());
    assert!(corrupt.validate().is_err());
}
