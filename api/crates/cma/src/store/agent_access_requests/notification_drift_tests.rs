use super::*;
use crate::store::sandboxes::sandbox_replacement_fields;
use layer2_core::sandbox::agent_io::{
    AgentResponseStatus, CommandClaim, CommandDispatch, CommandFailure,
};
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::sandbox::{SandboxWorkState, SandboxWorkStep};

async fn put_sandbox(store: &MongoSandboxStore, sandbox: &Sandbox) {
    sandbox.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(sandbox).unwrap()},
        )
        .await
        .unwrap();
}

async fn current_sandbox(store: &MongoSandboxStore, record: &AgentAccessRecord) -> Sandbox {
    store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap()
}

async fn insert_command(store: &MongoSandboxStore, command: &AdmittedAgentCommand) {
    command.validate().unwrap();
    store
        .agent_commands
        .insert_one(crate::store::agent_commands::command_document(command).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_pause_before_intent_releases_scope_lock_for_later_revocation() {
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let mut active = current_sandbox(&store, &record).await;
    let baseline = vec![
        ServiceId::new("chrono-llm-public"),
        ServiceId::new("ornn-api"),
    ];
    active.credential.reaches = baseline.clone();
    active.credential.reaches.push(ServiceId::new("api-github"));
    active.credential.reaches.sort();
    let mut paused = active.clone();
    paused
        .begin_pause(layer2_core::now_rfc3339(), None)
        .unwrap();
    paused.lifecycle.operation = None;
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    put_sandbox(&store, &paused).await;

    let failed = store.prepare_access_notification(lease).await.unwrap();
    assert_eq!(failed.phase, AgentAccessPhase::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("agent_notification_runtime_changed")
    );
    assert!(failed.lease.is_none());
    assert_eq!(failed.progress, record.progress);
    assert_eq!(current_sandbox(&store, &record).await, paused);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.read_access_request(&record.id).await.unwrap(),
        Some(failed)
    );
    assert_eq!(
        reopened.prepare_access_notification(lease).await,
        Err(AgentAccessError::Conflict)
    );

    // A later owner resume has advanced the lifecycle authority. The old
    // callback remains failed and cannot block a fresh optional-scope removal.
    active.lifecycle.next_authority = paused.lifecycle.next_authority + 1;
    active.revision = paused.revision + 1;
    active.updated_at = layer2_core::now_rfc3339();
    put_sandbox(&reopened, &active).await;
    let Some(AgentAccessDecision::Approved { authorization, .. }) = &record.decision else {
        panic!()
    };
    let saved = reopened
        .replace_agent_service_access(
            &record.requester.owner,
            &record.requester.agent_id,
            baseline.clone(),
            active.credential.effective_reaches(),
            &baseline,
            authorization.clone(),
            "revoke-after-pause",
        )
        .await
        .unwrap();
    let claimed = reopened
        .claim_access_request("revocation-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, saved.id);
    let updating = reopened
        .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(updating.phase, AgentAccessPhase::Updating);
    assert_eq!(
        current_sandbox(&reopened, &record)
            .await
            .credential
            .effective_reaches(),
        baseline
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_changed_runtime_after_intent_is_terminal_without_dispatch() {
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let mut sandbox = current_sandbox(&store, &record).await;
    sandbox.runtime.as_mut().unwrap().boot_id = Some("d".repeat(32));
    sandbox.revision += 1;
    put_sandbox(&store, &sandbox).await;
    let failed = store.prepare_access_notification(lease).await.unwrap();
    assert_eq!(failed.phase, AgentAccessPhase::Failed);
    assert_eq!(failed.progress, selected.progress);
    assert!(failed.lease.is_none());
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_prewrite_turn_retry_stops_after_lifecycle_changed() {
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    set_turn(&store, &record, Some(17)).await;
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let mut command = command(&selected);
    command.status = AgentResponseStatus::Failed;
    command.work.failure = Some(CommandFailure {
        code: CommandFailureCode::TargetTurnChanged,
        at: fixtures::NOW.into(),
    });
    insert_command(&store, &command).await;
    set_turn(&store, &record, None).await;
    let mut sandbox = current_sandbox(&store, &record).await;
    sandbox.lifecycle.next_authority += 1;
    sandbox.revision += 1;
    put_sandbox(&store, &sandbox).await;
    let failed = store.prepare_access_notification(lease).await.unwrap();
    assert_eq!(failed.phase, AgentAccessPhase::Failed);
    assert_eq!(
        failed.progress.notification_intent,
        selected.progress.notification_intent
    );
    assert_eq!(
        failed.progress.notification_command_id,
        Some(command.receipt.envelope.command_id)
    );
    assert_eq!(failed.notification_key(), selected.notification_key());
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_preserves_uncertain_and_confirmed_admission_after_lifecycle_changed() {
    for delivery in [
        AgentOperationDelivery::RecoveryRequired,
        AgentOperationDelivery::WriteConfirmed,
    ] {
        let (client, name, store, record) = notifying_fixture().await;
        let lease = record.lease.as_ref().unwrap();
        set_turn(&store, &record, Some(19)).await;
        let selected = store.prepare_access_notification(lease).await.unwrap();
        let mut command = command(&selected);
        command.status = AgentResponseStatus::InProgress;
        command.work.dispatch = Some(CommandDispatch {
            claim: CommandClaim {
                agent_id: record.requester.agent_id.clone(),
                command_id: command.receipt.envelope.command_id.clone(),
                holder: "notification-runtime".into(),
                fence: 1,
            },
            operation_sequence: 1,
            turn_sequence: 19,
            runtime_generation: record.requester.runtime.provision_generation.unwrap(),
            fenced_at: fixtures::NOW.into(),
            delivery,
        });
        insert_command(&store, &command).await;
        set_turn(&store, &record, None).await;
        let mut sandbox = current_sandbox(&store, &record).await;
        sandbox.lifecycle.next_authority += 1;
        sandbox.revision += 1;
        put_sandbox(&store, &sandbox).await;
        let recovered = store.prepare_access_notification(lease).await.unwrap();
        assert_eq!(recovered.phase, AgentAccessPhase::Notifying);
        assert_eq!(
            recovered.progress.notification_intent,
            selected.progress.notification_intent
        );
        assert_eq!(
            recovered.progress.notification_command_id,
            Some(command.receipt.envelope.command_id)
        );
        assert_eq!(recovered.notification_key(), selected.notification_key());
        assert_eq!(
            store.prepare_access_notification(lease).await.unwrap(),
            recovered
        );
        assert_eq!(
            store.agent_commands.count_documents(doc! {}).await.unwrap(),
            1
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_ordinary_rotation_remains_retryable() {
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let active = current_sandbox(&store, &record).await;
    let mut rotating = active.clone();
    rotating.status = SandboxStatus::CredentialRotating;
    rotating.work.schedule(
        SandboxWorkStep::MintRotatedCredential,
        layer2_core::now_rfc3339(),
    );
    rotating.revision += 1;
    put_sandbox(&store, &rotating).await;
    assert_eq!(
        store.prepare_access_notification(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(record.clone())
    );
    put_sandbox(&store, &active).await;
    let selected = store.prepare_access_notification(lease).await.unwrap();
    assert_eq!(selected.phase, AgentAccessPhase::Notifying);
    assert!(selected.progress.notification_intent.is_some());
    client.database(&name).drop().await.unwrap();
}
