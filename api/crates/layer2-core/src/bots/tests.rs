use super::*;
use crate::{profiles::*, Authorization, Caller, Capability, SubjectId};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

pub(super) struct Profiles;
#[async_trait::async_trait]
impl BotProfileResolver for Profiles {
    async fn resolve(&self, _: &SubjectId, request: &CreateBot) -> Result<BotProfiles, BotError> {
        let reference = &request.profile;
        let agent = AgentProfileContent {
            isolated_agent: false,
            model: Some("test".into()),
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        };
        let sandbox:SandboxProfileContent=serde_json::from_value(json!({"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":AgentProvider::default(),"model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]})).unwrap();
        let workspace = WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "bot-workspace".into(),
            },
            name: "bot-workspace".into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        };
        Ok(BotProfiles {
            event_worker_profile: None,
            resolved_model: "test".into(),
            agent: AgentProfileRevision {
                profile_id: reference.id.clone(),
                revision: reference.revision,
                digest: content_digest(&agent).unwrap(),
                content: agent,
                published_at: crate::now_rfc3339(),
            },
            sandbox: SandboxProfileRevision {
                profile_id: crate::SandboxProfileId("sbp_test".into()),
                revision: 1,
                digest: content_digest(&sandbox).unwrap(),
                content: sandbox,
                published_at: crate::now_rfc3339(),
            },
            workspace: WorkspaceProfileRevision {
                profile_id: crate::WorkspaceProfileId("wkp_test".into()),
                revision: 1,
                digest: content_digest(&workspace).unwrap(),
                content: workspace,
                published_at: crate::now_rfc3339(),
            },
        })
    }
}
pub(super) fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::ManageOwnBots]),
    )
}
pub(super) fn input() -> CreateBot {
    CreateBot {
        event_worker_profile: None,
        name: "test bot".into(),
        external_bot_id: uuid::Uuid::new_v4().to_string(),
        connection_ref: None,
        image: None,
        repository: None,
        inline_agent: None,
        profile: AgentProfileRef::new(crate::AgentProfileId("agp_test".into()), 1),
        sandbox_profile: None,
        workspace_profile: None,
        admission: BotAdmission::Scoped {
            senders: IdentityAdmission::Open,
            chats: IdentityAdmission::Open,
            groups: GroupAdmission::Deny,
        },
        reply: BotReply {
            auto_final: true,
            ack_after_ms: 2000,
        },
    }
}
fn fixture() -> (Arc<BotService>, Arc<InMemoryBotStore>) {
    let store = Arc::new(InMemoryBotStore::default());
    (
        BotService::new(store.clone(), Arc::new(Profiles), "registered".into()),
        store,
    )
}

#[tokio::test]
async fn concurrent_creation_is_owner_scoped_and_replays_the_original_snapshot() {
    let (service, _) = fixture();
    let owner = caller("owner");
    let request = input();
    let (a, b) = tokio::join!(
        service.create(&owner, "create-key", request.clone()),
        service.create(&owner, "create-key", request.clone())
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(service.list(&owner, None, 10).await.unwrap().len(), 1);
    let mut changed = request.clone();
    changed.name = "different".into();
    assert_eq!(
        service.create(&owner, "create-key", changed).await,
        Err(BotError::Conflict)
    );
    assert_eq!(
        service.get(&caller("other"), &a.bot_id).await,
        Err(BotError::NotFound)
    );
    let other = service
        .create(&caller("other"), "create-key", request)
        .await
        .unwrap();
    assert_ne!(other.bot_id, a.bot_id);
}
#[tokio::test]
async fn authority_and_invalid_policy_are_refused_before_admission() {
    let (service, store) = fixture();
    let visitor = Caller::new(SubjectId::new("owner"), Authorization::none());
    assert_eq!(
        service.create(&visitor, "key", input()).await,
        Err(BotError::Forbidden)
    );
    let mut request = input();
    request.admission = BotAdmission::Allowlist { sender_ids: vec![] };
    assert!(matches!(
        service.create(&caller("owner"), "key", request).await,
        Err(BotError::Invalid(_))
    ));
    assert!(store.pending(None, 10).await.unwrap().is_empty());
}
#[tokio::test]
async fn expired_creation_is_claimed_for_cleanup_and_old_writer_cannot_checkpoint() {
    let (service, store) = fixture();
    let bot = service
        .create(&caller("owner"), "key", input())
        .await
        .unwrap();
    let now = SystemTime::now();
    let mut old = service
        .claim(&bot.bot_id, "old", now)
        .await
        .unwrap()
        .unwrap();
    assert!(service
        .claim(&bot.bot_id, "new", now)
        .await
        .unwrap()
        .is_none());
    let current = service
        .claim(&bot.bot_id, "new", now + Duration::from_secs(121))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.work.as_ref().unwrap().operation,
        BotOperation::Cleanup
    );
    assert_eq!(
        service.checkpoint(&mut old, "old", "created_route").await,
        Err(BotError::Conflict)
    );
    assert_eq!(store.get(&bot.bot_id).await.unwrap().unwrap(), current);
}
#[tokio::test]
async fn incompatible_operations_cannot_overtake_claimed_work() {
    let (service, _) = fixture();
    let owner = caller("owner");
    let bot = service.create(&owner, "key", input()).await.unwrap();
    assert_eq!(
        service
            .admit(&owner, &bot.bot_id, BotOperation::Delete)
            .await,
        Err(BotError::Conflict)
    );
    let mut claimed = service
        .claim(&bot.bot_id, "worker", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    service
        .complete(
            &mut claimed,
            "worker",
            BotState::Failed {
                reason: "external_bot_missing".into(),
            },
        )
        .await
        .unwrap();
    let deleting = service
        .admit(&owner, &bot.bot_id, BotOperation::Delete)
        .await
        .unwrap();
    assert_eq!(deleting.state, BotState::Deleting);
    assert_eq!(
        service
            .admit(&owner, &bot.bot_id, BotOperation::Delete)
            .await
            .unwrap(),
        deleting
    );
    let mut claimed = service
        .claim(&bot.bot_id, "worker", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    service
        .complete(&mut claimed, "worker", BotState::Deleted)
        .await
        .unwrap();
    assert!(service.list(&owner, None, 10).await.unwrap().is_empty());
    assert_eq!(
        service
            .create(&owner, "key", bot.request)
            .await
            .unwrap()
            .state,
        BotState::Deleted
    );
}
#[tokio::test]
async fn immutable_pins_and_plaintext_hashes_cannot_be_written() {
    let (service, store) = fixture();
    let original = service
        .create(&caller("owner"), "key", input())
        .await
        .unwrap();
    for mutation in [0, 1, 2] {
        let mut changed = original.clone();
        changed.revision += 1;
        match mutation {
            0 => changed.owner = SubjectId::new("other"),
            1 => changed.profiles.resolved_model = "other".into(),
            _ => {
                changed.resources.credentials.agent_key_hash =
                    Some(uuid::Uuid::new_v4().to_string())
            }
        }
        assert!(store.replace(original.revision, &changed).await.is_err());
        assert_eq!(
            store.get(&original.bot_id).await.unwrap().unwrap(),
            original
        );
    }
}
