use std::{sync::Arc, time::Duration};

use layer2_core::bridge::*;
use layer2_core::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use layer2_core::resource_access::{
    CallerContext, CredentialContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    ResourceActor,
};
use layer2_core::{
    AgentProfileId, Authorization, Caller, Capability, SandboxProfileId, SubjectId,
    WorkspaceProfileId,
};

use super::*;
use crate::bridge::custody::BridgeCredentialStore;
use crate::store::AuthEncryptionKey;

fn custody_key() -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64(&base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [7_u8; 32],
    ))
    .unwrap()
}

async fn open_store(client: &Client, database: &str) -> MongoBridgeStore {
    MongoBridgeStore::open(client, database, "fixture-deployment", &custody_key())
        .await
        .unwrap()
}

async fn admit_custody(
    service: &BridgeService,
    store: &MongoBridgeStore,
    id: &BridgeId,
    revision: u64,
) {
    let management = service
        .management_lease(&caller(), None, id, revision)
        .await
        .unwrap();
    store
        .credentials
        .admit(&management, custody::material(&management))
        .await
        .unwrap();
}

struct Authority;
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        unreachable!()
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Authority {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: vec![BridgeRouteObservation {
                    route_id: "route-1".into(),
                    owner: owner.clone(),
                    bot_id: config.connector.bot_id.clone(),
                    key_id: config.connector.key_id.clone(),
                    enabled: true,
                }],
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 3,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: true,
                exact_profiles_available: true,
                existing_target: None,
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
    }
}
fn caller() -> CallerContext {
    CallerContext {
        caller: Caller::new(
            SubjectId::new("fixture-owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://nyx.example".into(),
            subject: SubjectId::new("fixture-owner"),
            credential_id: "fixture-login".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new("fixture-owner"),
        },
        memberships: vec![],
    }
}
fn owner() -> ResourceOwner {
    ResourceOwner::Person {
        subject: SubjectId::new("fixture-owner"),
    }
}
fn config() -> BridgeConfig {
    BridgeConfig {
        display_name: "Disposable bridge".into(),
        connector: BridgeConnector {
            issuer: "https://nyx.example".into(),
            base_url: "https://nyx.example".into(),
            bot_id: "existing-bot".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["route-1".into()],
            key_id: "key-1".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/callback".into(),
        },
        partition: BridgePartitionPolicy::Conversation,
        placement: BridgePlacementPolicy::NewSandbox {
            sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp-1".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp-1".into()), 1),
            agent_profile: AgentProfileRef::new(AgentProfileId("agp-1".into()), 1),
        },
        admission: BridgeAdmissionPolicy::All,
        reply: BridgeReplyPolicy::FinalResponse,
        limits: BridgeLimits {
            max_partitions: 2,
            max_sandboxes: 2,
            max_queued_messages: 5,
            max_messages_per_minute: 30,
            max_payload_bytes: 4096,
            inbox_retention_seconds: 3600,
            reply_secret_retention_seconds: 600,
        },
    }
}
fn service(store: Arc<MongoBridgeStore>) -> BridgeService {
    BridgeService::new(store, Arc::new(Authority), Arc::new(Authority))
}
fn change(
    id: &BridgeId,
    expected_revision: u64,
    key: &str,
    change: BridgeChange,
) -> BridgeMutation {
    BridgeMutation {
        id: id.clone(),
        expected_revision,
        idempotency_key: key.into(),
        change,
    }
}
async fn fixture() -> (Client, String, Arc<MongoBridgeStore>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("disposable Mongo contract replica URI is required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = format!("cma_bridge_contract_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(open_store(&client, &database).await);
    (client, database, store)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn restart_cas_race_and_key_rotation_preserve_original_binding_and_atomic_receipt() {
    let (client, database, store) = fixture().await;
    let svc = service(store.clone());
    let id = BridgeService::creation_id(&caller(), "create").unwrap();
    let created = svc
        .mutate(
            &caller(),
            None,
            change(
                &id,
                0,
                "create",
                BridgeChange::Create {
                    owner: owner(),
                    config: Box::new(config()),
                },
            ),
        )
        .await
        .unwrap();
    let reopened = Arc::new(open_store(&client, &database).await);
    let svc = service(reopened.clone());
    assert_eq!(
        svc.mutate(
            &caller(),
            None,
            change(
                &id,
                0,
                "create",
                BridgeChange::Create {
                    owner: owner(),
                    config: Box::new(config())
                }
            )
        )
        .await
        .unwrap(),
        created
    );
    admit_custody(&svc, &reopened, &id, 1).await;
    let context = caller();
    let (a, b) = tokio::join!(
        svc.mutate(
            &context,
            None,
            change(&id, 1, "enable-a", BridgeChange::Enable)
        ),
        svc.mutate(
            &context,
            None,
            change(&id, 1, "enable-b", BridgeChange::Enable)
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(a == Err(BridgeError::Conflict) || b == Err(BridgeError::Conflict));
    let pin = reopened
        .bridge(&id)
        .await
        .unwrap()
        .unwrap()
        .admission_pin()
        .unwrap();
    let mut rotated = config();
    rotated.connector.key_id = "key-2".into();
    rotated.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    svc.mutate(
        &context,
        None,
        change(
            &id,
            2,
            "disable-for-rotation",
            BridgeChange::Disable {
                accepted_work: AcceptedWorkPolicy::DrainOriginal,
            },
        ),
    )
    .await
    .unwrap();
    svc.mutate(
        &context,
        None,
        change(
            &id,
            3,
            "rotate",
            BridgeChange::Update {
                config: Box::new(rotated),
                previous_work: None,
            },
        ),
    )
    .await
    .unwrap();
    admit_custody(&svc, &reopened, &id, 4).await;
    svc.mutate(
        &context,
        None,
        change(&id, 4, "enable-rotated", BridgeChange::Enable),
    )
    .await
    .unwrap();
    svc.mutate(
        &context,
        None,
        change(
            &id,
            5,
            "retire",
            BridgeChange::Retire {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal,
            },
        ),
    )
    .await
    .unwrap();
    let reopened = open_store(&client, &database).await;
    let record = reopened.bridge(&id).await.unwrap().unwrap();
    assert_eq!(
        record.pinned(&pin).unwrap().config.connector.key_id,
        "key-1"
    );
    assert_eq!(record.current().unwrap().config.connector.key_id, "key-2");
    assert_eq!(record.admission_pin(), Err(BridgeError::AdmissionStopped));
    assert_eq!(reopened.receipts.count_documents(doc! {}).await.unwrap(), 6);
    assert_eq!(
        reopened.definitions.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owner_index_corruption_and_hidden_lookahead_refuse_public_inventory() {
    let (client, database, store) = fixture().await;
    let svc = service(store.clone());
    let a = BridgeId("brg_a".into());
    let b = BridgeId("brg_b".into());
    for (id, key) in [(&a, "create-a"), (&b, "create-b")] {
        svc.mutate(
            &caller(),
            None,
            change(
                id,
                0,
                key,
                BridgeChange::Create {
                    owner: owner(),
                    config: Box::new(config()),
                },
            ),
        )
        .await
        .unwrap();
    }
    let page = svc.list(&caller(), None, &owner(), None, 1).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_cursor, Some(a));
    store
        .definitions
        .update_one(
            doc! { "_id": b.as_str() },
            doc! { "$set": { "record.owner.subject": "unrelated-owner" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        svc.list(&caller(), None, &owner(), None, 1).await,
        Err(BridgeError::Corrupt)
    ));
    assert_eq!(store.bridge(&b).await, Err(BridgeError::Corrupt));
    assert_eq!(store.receipts.count_documents(doc! {}).await.unwrap(), 2);
    client.database(&database).drop().await.unwrap();
}

mod custody;
mod custody_proof;
mod inbox;

#[path = "../bridge_allocations/mongo_integration_tests.rs"]
mod allocations;
