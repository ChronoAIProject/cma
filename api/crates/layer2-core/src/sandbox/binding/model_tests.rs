use super::*;
use crate::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use crate::sandbox::{store::SandboxStoreError, InMemorySandboxStore};
use crate::SubjectId;
use serde_json::json;

const NOW: &str = "2026-09-22T00:00:00Z";

fn profile() -> BindingProfilePlan {
    let value = json!({
        "schema_version": 1,
        "metadata": {"name":"binding-test", "owner":{"kind":"human", "subject":"owner"}, "provider":"registered"},
        "model":{"access":"managed", "service":AgentProvider::default().connection_services().managed,
            "api_type":"supported", "model":"test", "thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"}, "context":{},
        "environment":{"size":{"cpu_cores":1, "memory_gib":2}, "workspace":{"type":"directory", "name":"project"}}
    });
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    map_profile(
        value,
        &digest,
        AgentProvider::default(),
        "supported",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap()
}

fn binding() -> ProviderBinding {
    ProviderBinding::new(
        "binding-one".into(),
        SubjectId::new("manager"),
        profile(),
        "a".repeat(64),
        NOW.into(),
    )
    .unwrap()
}

async fn ready(store: &InMemorySandboxStore) -> ProviderBinding {
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    value.state = BindingState::Ready;
    value.work = BindingWork::Idle;
    value.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), value.clone()).await.unwrap();
    value
}

#[test]
fn metadata_reconfiguration_preserves_generation_and_never_schedules_cleanup() {
    let mut value = binding();
    value.state = BindingState::Ready;
    value.work = BindingWork::Idle;
    let mut metadata = value.profile.clone();
    metadata.name = "Renamed binding".into();
    metadata.profile_digest = format!("sha256:{}", "b".repeat(64));
    let changed = value
        .reconfigure(metadata, value.key_sha256.clone(), NOW.into())
        .unwrap();
    assert_eq!(changed.sandbox_id, value.sandbox_id);
    assert_eq!(changed.workspace_id, value.workspace_id);
    assert_eq!(changed.generation, value.generation);
    assert_eq!(changed.state, BindingState::Ready);
    assert_eq!(changed.work, BindingWork::Idle);
    assert!(changed.previous.is_none());
    super::store::validate_binding_write(Some(&value), Some(value.revision), &changed).unwrap();

    for field in [
        "prompt",
        "skills",
        "services",
        "resources",
        "workspace",
        "key",
        "affinity",
    ] {
        let mut profile = value.profile.clone();
        let mut key = value.key_sha256.clone();
        match field {
            "prompt" => profile.agent.system_prompt = "Updated instruction".into(),
            "skills" => profile.agent.skills.push(crate::profiles::SkillRef {
                source: crate::profiles::SkillSource::Ornn,
                id: crate::SkillId::new("new-skill"),
                version: Some("1.0".into()),
            }),
            "services" => profile
                .environment
                .as_mut()
                .unwrap()
                .sandbox
                .nyxid_services
                .push(crate::ServiceId::new("extra")),
            "resources" => profile.environment.as_mut().unwrap().sandbox.cpu_millis += 250,
            "workspace" => profile
                .environment
                .as_mut()
                .unwrap()
                .workspace
                .setup_commands
                .push("setup-new".into()),
            "affinity" => profile.session_affinity = SessionAffinity::SourceUser,
            _ => key = "c".repeat(64),
        }
        assert!(
            value.reconfigure(profile, key, NOW.into()).is_err(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn binding_cas_and_conversation_identity_survive_competing_creates_and_retirement() {
    let store = InMemorySandboxStore::default();
    let value = ready(&store).await;
    let first = BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    let second = BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    let (a, b) = tokio::join!(
        store.save_binding_conversation(value.revision, None, first.clone()),
        store.save_binding_conversation(value.revision, None, second)
    );
    assert!(a.is_ok());
    assert_eq!(b, Err(SandboxStoreError::Conflict));
    let replay = store
        .binding_conversation(&value.binding_id, value.generation, "conversation_one")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.agent_id, first.agent_id);
    assert_eq!(replay.worktree_id, first.worktree_id);
    let mut retired = value.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(value.revision), retired.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .save_binding(Some(value.revision), retired.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let late = BindingConversation::new(&value, "conversation_two".into(), NOW.into()).unwrap();
    assert_eq!(
        store
            .save_binding_conversation(value.revision, None, late.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .save_binding_conversation(retired.revision, None, late)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let mut cleanup = first.clone();
    cleanup.state = BindingConversationState::Retiring;
    cleanup.revision += 1;
    store
        .save_binding_conversation(retired.revision, Some(first.revision), cleanup.clone())
        .await
        .unwrap();
    let expected = cleanup.revision;
    cleanup.state = BindingConversationState::Retired;
    cleanup.revision += 1;
    store
        .save_binding_conversation(retired.revision, Some(expected), cleanup.clone())
        .await
        .unwrap();
    let mut resurrected = cleanup.clone();
    resurrected.state = BindingConversationState::Ready;
    resurrected.revision += 1;
    assert_eq!(
        store
            .save_binding_conversation(retired.revision, Some(cleanup.revision), resurrected)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .binding_conversations(&value.binding_id, 1, None, 10)
            .await
            .unwrap(),
        [cleanup]
    );
    assert!(store
        .pending_binding_conversations(None, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn binding_storage_fences_owner_lineage_and_conversation_identifiers() {
    let store = InMemorySandboxStore::default();
    let value = ready(&store).await;
    for invalid in [
        "short",
        "conversation/escape",
        "conversation space",
        "éééééééé",
    ] {
        assert!(BindingConversation::new(&value, invalid.into(), NOW.into()).is_err());
    }
    assert!(BindingConversation::new(&value, "a".repeat(129), NOW.into()).is_err());
    assert!(BindingConversation::new(&value, "a".repeat(128), NOW.into()).is_ok());
    let mut conversation =
        BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    conversation.owner = SubjectId::new("other-owner");
    assert_eq!(
        store
            .save_binding_conversation(value.revision, None, conversation)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let mut changed = value.clone();
    changed.manager = SubjectId::new("other-manager");
    changed.advance_revision(NOW.into()).unwrap();
    assert_eq!(
        store.save_binding(Some(value.revision), changed).await,
        Err(SandboxStoreError::Conflict)
    );
    assert!(store.pending_bindings(None, 0).await.is_err());
    assert!(store
        .binding_conversations(&value.binding_id, 1, None, 201)
        .await
        .is_err());
}
