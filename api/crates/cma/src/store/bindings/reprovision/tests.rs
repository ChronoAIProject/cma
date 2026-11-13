use super::*;
use crate::sandbox::SandboxCredentialCustody;
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
use layer2_core::profiles::content_digest;
use layer2_core::sandbox::binding::{BindingCredentialCustody, BindingState, BindingWork};
use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};
use layer2_core::sandbox::{
    Sandbox, SandboxHome, SandboxStatus, SandboxWorkStep, Workspace, WorkspaceStatus,
    WorkspaceWorkStep,
};
use std::sync::Arc;

struct Data {
    binding: ProviderBinding,
    sandbox: Sandbox,
    workspace: Workspace,
    key: ProvidedCredential,
    intent: BindingPreparationIntent,
}
impl Data {
    fn new() -> Self {
        let now = layer2_core::now_rfc3339();
        let seconds = layer2_core::seconds_from_rfc3339(&now).unwrap() as i64;
        let key =
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
        let mut binding = super::super::mongo_tests::binding();
        binding.state = BindingState::Failed {
            reason: "original home explicitly retired".into(),
        };
        binding.work = BindingWork::Idle;
        binding.key_sha256 = key.sha256();
        binding.generation = 3;
        binding.revision = 14;
        let mut sandbox = super::super::admission_tests::sandbox(&binding);
        sandbox.status = SandboxStatus::Retired;
        sandbox.retired_at = Some(super::super::mongo_tests::NOW.into());
        sandbox
            .work
            .schedule(SandboxWorkStep::WaitForPurge, now.clone());
        let mut home = SandboxHome::reserve(
            &sandbox.id,
            layer3_adapter::HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: "seed".into(),
                template_generation: "original".into(),
            },
        )
        .unwrap();
        home.released = true;
        sandbox.lifecycle.home = Some(Box::new(home));
        let mut workspace = super::super::admission_tests::workspace(&binding);
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("a".repeat(40));
        workspace.ready_at = Some(now);
        workspace.work.schedule(
            WorkspaceWorkStep::None,
            super::super::mongo_tests::NOW.into(),
        );
        let mut data = Self {
            binding,
            sandbox,
            workspace,
            key,
            intent: BindingPreparationIntent {
                version: 1,
                admission_id: "ordinary-event-one".into(),
                target_id: "consumer-one".into(),
                request_hash: "a".repeat(64),
                binding_id: String::new(),
                owner: layer2_core::SubjectId::new("owner"),
                manager: layer2_core::SubjectId::new("manager"),
                generation: 3,
                binding_revision: 14,
                binding_digest: String::new(),
                sandbox_digest: String::new(),
                workspace_digest: String::new(),
                admitted_at: seconds - 1,
                expires_at: seconds + 600,
            },
        };
        data.refresh();
        data
    }
    fn refresh(&mut self) {
        self.intent.binding_id = self.binding.binding_id.clone();
        self.intent.binding_digest = content_digest(&self.binding).unwrap();
        self.intent.sandbox_digest = content_digest(&self.sandbox).unwrap();
        self.intent.workspace_digest = content_digest(&self.workspace).unwrap();
    }
    fn reserve(&self) -> Result<BindingPreparationReceipt, layer2_core::CoreError> {
        self.binding.prepare_retired_hierarchy(
            &self.intent,
            &self.sandbox,
            &self.workspace,
            &self.key,
            layer2_core::now_rfc3339(),
        )
    }
}

#[test]
fn preparation_preserves_archive_and_requires_exact_generation_for_new_submissions() {
    let data = Data::new();
    let receipt = data.reserve().unwrap();
    assert_eq!(receipt.previous, data.binding);
    assert_ne!(receipt.reserved.sandbox_id, data.sandbox.id);
    assert_ne!(receipt.reserved.workspace_id, data.workspace.id);
    assert_eq!(receipt.reserved.generation, 4);
    assert_eq!(receipt.reserved.work, BindingWork::ProvisionSandbox);
    assert_eq!(receipt.reserved.profile, data.binding.profile);
    assert!(receipt.reserved.previous.is_none());
    assert!(receipt.reserved.require_generation(Some(4)).is_ok());
    assert!(receipt.reserved.require_generation(Some(3)).is_err());
    assert!(receipt.reserved.require_generation(None).is_err());
    assert!(data.binding.require_generation(None).is_ok());
    assert!(validate_binding_write(
        Some(&data.binding),
        Some(data.binding.revision),
        &receipt.reserved
    )
    .is_err());
    let mut unauthorized = super::super::mongo_tests::binding();
    unauthorized.generation_guard = Some(layer2_core::sandbox::binding::BindingGenerationGuard {
        generation: 1,
        operation_id: receipt.operation_id.clone(),
    });
    assert!(validate_binding_write(None, None, &unauthorized).is_err());
    let old = serde_json::to_value(&data.binding).unwrap();
    assert!(old.get("generation_guard").is_none());
    assert_eq!(
        serde_json::from_value::<ProviderBinding>(old).unwrap(),
        data.binding
    );
    let bytes = serde_json::to_vec(&receipt).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(data.key.expose()));
    assert_eq!(
        serde_json::from_slice::<BindingPreparationReceipt>(&bytes).unwrap(),
        receipt
    );
}

#[test]
fn preparation_accepts_ready_idle_only_after_the_original_home_is_fully_retired() {
    let mut data = Data::new();
    data.binding.state = BindingState::Ready;
    data.sandbox.work.execution = Some(Default::default());
    data.workspace.work.execution = Some(Default::default());
    data.refresh();
    let receipt = data.reserve().unwrap();
    assert_eq!(receipt.previous.state, BindingState::Ready);
    assert_eq!(receipt.reserved.generation, data.binding.generation + 1);
    for work in [
        BindingWork::DeliverCredential,
        BindingWork::ProvisionSandbox,
    ] {
        let mut busy = data.binding.clone();
        busy.state = BindingState::Provisioning;
        busy.work = work;
        data.binding = busy;
        data.refresh();
        assert!(data.reserve().is_err());
    }
}

#[test]
fn preparation_refuses_every_recoverable_status_and_unreleased_home() {
    for status in [
        SandboxStatus::Provisioning,
        SandboxStatus::Active,
        SandboxStatus::Paused,
        SandboxStatus::Pausing,
        SandboxStatus::Reviving,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
    ] {
        let mut data = Data::new();
        data.sandbox.status = status;
        data.refresh();
        assert!(data.reserve().is_err(), "accepted {status:?}");
    }
    let mut data = Data::new();
    data.sandbox.lifecycle.home.as_mut().unwrap().released = false;
    data.refresh();
    assert!(data.reserve().is_err());
    let mut data = Data::new();
    data.binding.state = BindingState::Retired;
    data.refresh();
    assert!(data.reserve().is_err());
}

#[test]
fn preparation_rejects_stale_owner_key_event_and_changed_receipt() {
    for field in [
        "owner",
        "manager",
        "key",
        "binding_digest",
        "sandbox_digest",
        "workspace_digest",
        "generation",
        "revision",
        "old_event",
        "expired",
    ] {
        let mut data = Data::new();
        match field {
            "owner" => data.intent.owner = layer2_core::SubjectId::new("other"),
            "manager" => data.intent.manager = layer2_core::SubjectId::new("other"),
            "key" => data.key = ProvidedCredential::new("nyx_wrong".into()).unwrap(),
            "binding_digest" => data.intent.binding_digest = "f".repeat(64),
            "sandbox_digest" => data.intent.sandbox_digest = "f".repeat(64),
            "workspace_digest" => data.intent.workspace_digest = "f".repeat(64),
            "generation" => data.intent.generation += 1,
            "revision" => data.intent.binding_revision += 1,
            "old_event" => {
                data.intent.admitted_at =
                    layer2_core::seconds_from_rfc3339(super::super::mongo_tests::NOW).unwrap()
                        as i64
            }
            "expired" => data.intent.expires_at = data.intent.admitted_at,
            _ => unreachable!(),
        }
        assert!(data.reserve().is_err(), "accepted {field}");
    }
    let data = Data::new();
    let mut receipt = data.reserve().unwrap();
    receipt.reserved.key_sha256 = "f".repeat(64);
    assert!(receipt.validate().is_err());
    let mut receipt = data.reserve().unwrap();
    receipt.reserved.profile.name = "changed".into();
    assert!(receipt.validate().is_err());
}

async fn fixture() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    Arc<MongoSandboxCredentialStore>,
    Arc<MongoBindingCredentials>,
    Data,
) {
    let (client, database, store) = super::super::mongo_tests::fixture().await;
    let store = Arc::new(store);
    let encryption =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let custody = Arc::new(
        MongoSandboxCredentialStore::open(
            &client,
            &database,
            "binding-preparation-test",
            &encryption,
        )
        .await
        .unwrap(),
    );
    let controller = MongoBindingCredentials::new(store.clone(), custody.clone());
    let data = Data::new();
    store
        .bindings
        .records
        .insert_one(binding_document(&data.binding).unwrap())
        .await
        .unwrap();
    store.create_sandbox(data.sandbox.clone()).await.unwrap();
    store
        .create_workspace(data.workspace.clone())
        .await
        .unwrap();
    (client, database, store, custody, controller, data)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_concurrent_exact_replays_create_one_hierarchy_and_preserve_terminal_data(
) {
    let (client, database, store, custody, controller, data) = fixture().await;
    let mut old = data.binding.clone();
    old.state = BindingState::Ready;
    old.generation = 2;
    let mut conversation = BindingConversation::new(
        &old,
        "archived_conversation".into(),
        super::super::mongo_tests::NOW.into(),
    )
    .unwrap();
    conversation.state = layer2_core::sandbox::binding::BindingConversationState::Retired;
    let archived = conversation_document(&conversation).unwrap();
    store
        .bindings
        .conversations
        .insert_one(archived.clone())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        controller.prepare_retired_binding(&data.intent, &data.key),
        controller.prepare_retired_binding(&data.intent, &data.key)
    );
    assert!(a.is_ok() || b.is_ok());
    let receipt = controller
        .prepare_retired_binding(&data.intent, &data.key)
        .await
        .unwrap();
    for replay in [a, b].into_iter().filter_map(Result::ok) {
        assert_eq!(replay, receipt);
    }
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.binding(&data.binding.binding_id).await.unwrap(),
        Some(receipt.reserved.clone())
    );
    assert_eq!(
        store.get_sandbox(&data.sandbox.id).await.unwrap(),
        Some(data.sandbox)
    );
    assert_eq!(
        store.get_workspace(&data.workspace.id).await.unwrap(),
        Some(data.workspace)
    );
    assert_eq!(
        store
            .bindings
            .conversations
            .find_one(doc! {"_id":conversation_storage_key(&conversation)})
            .await
            .unwrap(),
        Some(archived)
    );
    assert!(store
        .get_sandbox(&receipt.reserved.sandbox_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        custody
            .get(&receipt.reserved.sandbox_id, 1)
            .await
            .unwrap()
            .unwrap()
            .value
            .expose(),
        data.key.expose()
    );
    assert!(custody
        .get(&receipt.previous.sandbox_id, 1)
        .await
        .unwrap()
        .is_none());
    let mut changed = data.intent.clone();
    changed.request_hash = "b".repeat(64);
    assert!(controller
        .prepare_retired_binding(&changed, &data.key)
        .await
        .is_err());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_distinct_events_cannot_allocate_twice_for_one_generation() {
    let (client, database, store, _, controller, data) = fixture().await;
    let mut other = data.intent.clone();
    other.admission_id = "second-fresh-event".into();
    let (a, b) = tokio::join!(
        controller.prepare_retired_binding(&data.intent, &data.key),
        controller.prepare_retired_binding(&other, &data.key)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let (winner, loser) = if let Ok(winner) = a {
        (winner, &other)
    } else {
        (b.unwrap(), &data.intent)
    };
    assert!(controller
        .prepare_retired_binding(loser, &data.key)
        .await
        .is_err());
    let mut failed = winner.reserved.clone();
    failed.state = BindingState::Failed {
        reason: "subsequent failure".into(),
    };
    failed.work = BindingWork::Idle;
    failed.advance_revision(layer2_core::now_rfc3339()).unwrap();
    store
        .save_binding(Some(winner.reserved.revision), failed.clone())
        .await
        .unwrap();
    assert_eq!(
        controller
            .prepare_retired_binding(&winner.intent, &data.key)
            .await
            .unwrap(),
        winner
    );
    assert_eq!(
        store.binding(&failed.binding_id).await.unwrap(),
        Some(failed)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_lost_ack_replays_original_receipt_after_reopen() {
    let (client, database, store, custody, controller, data) = fixture().await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let committed = controller
        .apply_preparation(&data.intent, &data.key, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap(); // Simulate a lost application acknowledgement.
    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let replacement = MongoBindingCredentials::new(reopened, custody);
    assert_eq!(
        replacement
            .prepare_retired_binding(&data.intent, &data.key)
            .await
            .unwrap(),
        committed
    );
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[path = "http_tests.rs"]
mod http_tests;

#[path = "lifecycle_http_tests.rs"]
mod lifecycle_http_tests;

#[path = "operator_stop_tests.rs"]
mod operator_stop_tests;

#[path = "fencing_tests.rs"]
mod fencing;
