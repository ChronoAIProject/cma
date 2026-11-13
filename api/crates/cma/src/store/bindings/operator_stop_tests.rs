use super::admission_tests::{sandbox, workspace};
use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::admission::AdmissionControlStore;
use layer2_core::profiles::{content_digest, AgentProfileSnapshot};
use layer2_core::sandbox::binding::{
    BindingAdmissionFence, BindingConversationState, BindingState, BindingWork,
};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentProfileId, RuntimeCapabilities, RuntimeCapability, SubjectId};

async fn stop(client: &mongodb::Client, database: &str) {
    crate::store::admission_control::MongoAdmissionControlStore::open(client, database)
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain incident",
            true,
            100,
        )
        .await
        .unwrap();
}

async fn advance(
    store: &MongoSandboxStore,
    value: &mut ProviderBinding,
    state: BindingState,
    work: BindingWork,
) {
    let revision = value.revision;
    value.state = state;
    value.work = work;
    value.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(revision), value.clone())
        .await
        .unwrap();
}

async fn ready(store: &MongoSandboxStore) -> ProviderBinding {
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    advance(store, &mut value, BindingState::Ready, BindingWork::Idle).await;
    value
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn operator_stop_refuses_a_fresh_binding_hierarchy_with_metering_off() {
    let (client, database, store) = fixture().await;
    stop(&client, &database).await;
    let second = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        second.save_binding(None, binding()).await,
        Err(SandboxStoreError::AdmissionPaused)
    );
    assert_eq!(
        store
            .bindings
            .records
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(store.workspaces.count_documents(doc! {}).await.unwrap(), 0);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn operator_stop_refuses_a_fresh_binding_conversation_without_partial_parent_writes() {
    let (client, database, store) = fixture().await;
    let value = ready(&store).await;
    let before = store
        .bindings
        .records
        .find_one(doc! {"_id": &value.binding_id})
        .await
        .unwrap();
    stop(&client, &database).await;
    let second = MongoSandboxStore::open(&client, &database).await.unwrap();
    let conversation =
        BindingConversation::new(&value, "fresh-conversation".into(), NOW.into()).unwrap();
    assert_eq!(
        second
            .save_binding_conversation(value.revision, None, conversation)
            .await,
        Err(SandboxStoreError::AdmissionPaused)
    );
    assert_eq!(
        store
            .bindings
            .records
            .find_one(doc! {"_id": &value.binding_id})
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .bindings
            .conversations
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn operator_stop_keeps_new_retirement_tombstones_durable_and_parent_fenced() {
    let (client, database, store) = fixture().await;
    let value = ready(&store).await;
    stop(&client, &database).await;
    let mut tombstone =
        BindingConversation::new(&value, "deleted-conversation".into(), NOW.into()).unwrap();
    tombstone.state = BindingConversationState::Retiring;
    assert_eq!(
        store
            .save_binding_conversation(value.revision - 1, None, tombstone.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .bindings
            .conversations
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    store
        .save_binding_conversation(value.revision, None, tombstone.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .save_binding_conversation(value.revision, None, tombstone.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened
            .binding_conversation(
                &value.binding_id,
                value.generation,
                &tombstone.conversation_id
            )
            .await
            .unwrap(),
        Some(tombstone.clone())
    );
    assert_eq!(
        reopened
            .pending_binding_conversations(None, 100)
            .await
            .unwrap(),
        [tombstone.clone()]
    );
    assert!(store
        .get_agent(&tombstone.agent_id)
        .await
        .unwrap()
        .is_none());
    let mut resurrected = tombstone;
    resurrected.state = BindingConversationState::Provisioning;
    resurrected.revision += 1;
    assert_eq!(
        reopened
            .save_binding_conversation(value.revision, Some(1), resurrected)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .bindings
            .conversations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_binding_background_hierarchy_continues_after_operator_stop() {
    let (client, database, store) = fixture().await;
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    advance(
        &store,
        &mut value,
        BindingState::Provisioning,
        BindingWork::ProvisionSandbox,
    )
    .await;
    stop(&client, &database).await;
    let parent = sandbox(&value);
    let mut admission = SandboxAdmission::new(
        parent.clone(),
        SandboxAdmissionName::Exact(parent.name.clone()),
        3,
        NOW.into(),
    )
    .unwrap();
    admission.binding = Some(BindingAdmissionFence::environment(&value));
    store.admit_sandbox(admission).await.unwrap();
    let mut parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    parent.status = SandboxStatus::Active;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    parent.revision += 1;
    store.save_sandbox(1, parent.clone()).await.unwrap();
    advance(
        &store,
        &mut value,
        BindingState::Provisioning,
        BindingWork::ProvisionWorkspace,
    )
    .await;
    let child = workspace(&value);
    store
        .admit_workspace(WorkspaceAdmission {
            binding: Some(BindingAdmissionFence::environment(&value)),
            workspace: child.clone(),
            kind: WorkspaceAdmissionKind::Manual,
            event_at: NOW.into(),
        })
        .await
        .unwrap();
    let mut child = store.get_workspace(&child.id).await.unwrap().unwrap();
    child.status = WorkspaceStatus::Ready;
    child.resolved_commit = Some("a".repeat(40));
    child.ready_at = Some(NOW.into());
    child.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    child.revision += 1;
    store.save_workspace(1, child).await.unwrap();
    advance(&store, &mut value, BindingState::Ready, BindingWork::Idle).await;
    assert_eq!(store.binding(&value.binding_id).await.unwrap(), Some(value));
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(store.workspaces.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_binding_conversation_provisions_once_and_remains_observable_after_stop() {
    let (client, database, store) = fixture().await;
    let value = ready(&store).await;
    let mut parent = sandbox(&value);
    parent.status = SandboxStatus::Active;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(parent).await.unwrap();
    let mut child = workspace(&value);
    child.status = WorkspaceStatus::Ready;
    child.resolved_commit = Some("a".repeat(40));
    child.ready_at = Some(NOW.into());
    child.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    store.create_workspace(child.clone()).await.unwrap();
    let mut conversation =
        BindingConversation::new(&value, "admitted-conversation".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(value.revision, None, conversation.clone())
        .await
        .unwrap();
    stop(&client, &database).await;
    let content = value.profile.agent.clone();
    let admission = AgentAdmission {
        binding: Some(BindingAdmissionFence::conversation(&value, &conversation)),
        created_by: None,
        owner: value.owner.clone(),
        workspace_id: child.id,
        agent_id: conversation.agent_id.clone(),
        worktree_id: conversation.worktree_id.clone(),
        profile: AgentProfileSnapshot {
            id: AgentProfileId::generate(),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: conversation.creation_key().unwrap(),
        event_at: NOW.into(),
    };
    let created = store.admit_agent(admission.clone()).await.unwrap();
    assert_eq!(created.creation().agent.id, conversation.agent_id);
    assert!(matches!(
        store.admit_agent(admission).await,
        Ok(AgentAdmissionResult::Existing(_))
    ));
    let revision = conversation.revision;
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    store
        .save_binding_conversation(value.revision, Some(revision), conversation.clone())
        .await
        .unwrap();
    let second = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        second.binding(&value.binding_id).await.unwrap(),
        Some(value.clone())
    );
    assert_eq!(
        second
            .binding_conversation(
                &value.binding_id,
                value.generation,
                &conversation.conversation_id
            )
            .await
            .unwrap(),
        Some(conversation)
    );
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store
            .bindings
            .conversations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn review_round_one_binding_save_keeps_single_document_insert_and_update() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command);
        }
    }));
    let client = mongodb::Client::with_options(options).unwrap();
    let database = format!("cma_binding_single_write_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    trace.lock().unwrap().clear();
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    let commands = trace.lock().unwrap().clone();
    assert!(commands
        .iter()
        .all(|command| !command.contains_key("autocommit")));
    let insert = commands
        .iter()
        .position(|command| command.get_str("insert").ok() == Some(BINDINGS))
        .unwrap();
    let read = &commands[insert - 1];
    assert_eq!(read.get_str("find").ok(), Some("platform_settings"));
    assert_eq!(
        read.get_document("readConcern")
            .unwrap()
            .get_str("level")
            .ok(),
        Some("majority")
    );
    trace.lock().unwrap().clear();
    advance(&store, &mut value, BindingState::Ready, BindingWork::Idle).await;
    let commands = trace.lock().unwrap().clone();
    assert!(commands
        .iter()
        .all(|command| !command.contains_key("autocommit")));
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.get_str("update").ok() == Some(BINDINGS))
            .count(),
        1
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.get_str("find").ok() == Some("platform_settings")),
        "an admitted binding update remains a continuation"
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unreadable_control_pauses_new_bindings_but_allows_admitted_updates() {
    let (client, database, store) = fixture().await;
    let mut admitted = binding();
    store.save_binding(None, admitted.clone()).await.unwrap();
    stop(&client, &database).await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_control": {"future_field": true}}},
        )
        .await
        .unwrap();
    let mut fresh = binding();
    fresh.binding_id = "different-binding".into();
    fresh.key_sha256 = "d".repeat(64);
    assert_eq!(
        store.save_binding(None, fresh).await,
        Err(SandboxStoreError::AdmissionPaused)
    );
    advance(
        &store,
        &mut admitted,
        BindingState::Ready,
        BindingWork::Idle,
    )
    .await;
    assert_eq!(
        store.binding(&admitted.binding_id).await.unwrap(),
        Some(admitted)
    );
    client.database(&database).drop().await.unwrap();
}
