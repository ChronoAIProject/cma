use super::*;
use crate::auth::persistence::SecretValue;
use crate::config::ProviderToolRelay;
use crate::sandbox::{CredentialMaterial, SandboxCredentialCustody};
use crate::store::bindings::mongo_tests;
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
use base64::Engine as _;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use sha2::{Digest, Sha256};

#[path = "relay_reconnect_tests.rs"]
mod reconnect;

const ORIGIN: &str = "https://cma.example.test/";
const NATIVE: &str = "native-conversation";
const ISSUER: &str = "nyx_test_issuer_kept_only_in_server_custody";

struct Fixture {
    client: mongodb::Client,
    database: String,
    store: Arc<MongoSandboxStore>,
    source: Arc<MongoProviderToolSource>,
    binding: ProviderBinding,
    conversation: BindingConversation,
    request: AgentOpenRequest,
}

impl Fixture {
    async fn new() -> Self {
        let (client, database, store) = mongo_tests::fixture().await;
        let (binding, conversation, request) =
            tests::setup_with_key(&store, format!("{:x}", Sha256::digest(ISSUER))).await;
        let key = AuthEncryptionKey::from_base64(
            &base64::engine::general_purpose::STANDARD.encode([9_u8; 32]),
        )
        .unwrap();
        let custody = Arc::new(
            MongoSandboxCredentialStore::open(&client, &database, "relay-test", &key)
                .await
                .unwrap(),
        );
        let sandbox = store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        custody
            .insert(CredentialMaterial {
                github_route: None,
                sandbox_id: sandbox.id,
                owner: sandbox.owner,
                authority: sandbox.credential.current.unwrap(),
                source: sandbox.credential.source,
                value: SecretValue::new(ISSUER.into()),
            })
            .await
            .unwrap();
        let config = ProviderToolRelay {
            bindings: vec![binding.binding_id.clone()],
            from_endpoint: binding.tool_endpoint.clone().unwrap(),
            upstream: format!("{ORIGIN}event-gateway/mcp"),
        };
        let store = Arc::new(store);
        let source =
            MongoProviderToolSource::with_relay(store.clone(), custody, Some(config), ORIGIN, &key)
                .unwrap();
        Self {
            client,
            database,
            store,
            source,
            binding,
            conversation,
            request,
        }
    }

    async fn open(&self, source: &MongoProviderToolSource) -> HostedProviderToolBinding {
        let mut attachment = source
            .resolve(&RuntimeHandle("provider-runtime".into()), &self.request, 1)
            .await
            .unwrap()
            .unwrap();
        attachment
            .observe(layer3_adapter::ToolProxyStatus {
                request: attachment.scope().3.clone(),
                boot_id: "b".repeat(32),
            })
            .unwrap();
        attachment
    }

    fn capability(&self, attachment: &HostedProviderToolBinding) -> String {
        self.source
            .relay
            .as_ref()
            .unwrap()
            .capability(attachment.provenance_sha256())
    }

    async fn publish(&self) -> Agent {
        let mut tree = self
            .store
            .get_worktree(&self.conversation.worktree_id)
            .await
            .unwrap()
            .unwrap();
        let revision = tree.revision;
        tree.status = WorktreeStatus::Ready;
        tree.revision += 1;
        self.store.save_worktree(revision, tree).await.unwrap();
        let mut agent = self
            .store
            .get_agent(&self.conversation.agent_id)
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.status = AgentStatus::Ready;
        agent.work.step = AgentWorkStep::None;
        agent.work.lease = None;
        agent.work.next_fence = 2;
        agent.conversation.conversation_id = Some(NATIVE.into());
        agent.conversation.runtime_generation = Some(1);
        agent.revision += 1;
        self.store
            .save_agent(revision, agent.clone())
            .await
            .unwrap();
        agent
    }

    async fn idle_intent(&self, mut agent: Agent) {
        let revision = agent.revision;
        let mut settings = agent.settings();
        settings.model = "test".into();
        agent.conversation.settings = Some(settings.clone());
        self.store.agents.update_one(doc!{"_id":agent.id.as_str()},
            doc!{"$set":{"record.conversation.settings":record_bson(&agent.conversation.settings).unwrap()}}).await.unwrap();
        agent.conversation.last_seq = 1;
        agent.conversation.operation = Some(
            AgentOperationRecord::new(
                1,
                1,
                1,
                AgentOperationIntent::RefreshContext {
                    revision: 2,
                    service_origin: Some(ORIGIN.into()),
                    readiness_receipt: "a".repeat(64),
                    guidance: "Managed guidance".into(),
                    previous_guidance: vec![],
                    settings,
                    working_directory: None,
                },
                layer2_core::now_rfc3339(),
            )
            .unwrap(),
        );
        agent.revision += 1;
        self.store.save_agent(revision, agent).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_relay_requires_confirmed_native_publication_and_fresh_custody() {
    let f = Fixture::new().await;
    let attachment = f.open(&f.source).await;
    assert_eq!(attachment.transport_origin(), Some(ORIGIN));
    let bearer = f.capability(&attachment);
    assert!(!bearer.contains(ISSUER));
    let setup = f.source.authenticate_relay(&bearer, true).await.unwrap();
    assert_eq!(setup.credential.expose(), ISSUER);
    assert_eq!(
        setup.upstream.as_str(),
        format!("{ORIGIN}event-gateway/mcp")
    );
    assert!(f.source.authenticate_relay(&bearer, false).await.is_err());
    f.source.confirm_open(&attachment, NATIVE).await.unwrap();
    // Confirmed discovery can occur before PublishReady; real effects cannot.
    f.source.authenticate_relay(&bearer, true).await.unwrap();
    assert!(f.source.authenticate_relay(&bearer, false).await.is_err());
    f.publish().await;
    f.source.authenticate_relay(&bearer, false).await.unwrap();
    for invalid in [
        ISSUER.into(),
        bearer.replace("cma_pt_", "cma_at_"),
        format!("{bearer}0"),
        format!("cma_pt_{}_{}", "0".repeat(64), "0".repeat(64)),
    ] {
        assert!(f.source.authenticate_relay(&invalid, true).await.is_err());
    }
    let db = f.client.database(&f.database);
    for (collection, filter, mutation) in [
        (
            "agents",
            doc! {"_id":f.conversation.agent_id.as_str()},
            doc! {"record.conversation.conversation_id":"other-native"},
        ),
        (
            "agents",
            doc! {"_id":f.conversation.agent_id.as_str()},
            doc! {"record.status":"deleted"},
        ),
        (
            "sandboxes",
            doc! {"_id":f.binding.sandbox_id.as_str()},
            doc! {"record.owner":"other-owner"},
        ),
        (
            "sandboxes",
            doc! {"_id":f.binding.sandbox_id.as_str()},
            doc! {"record.runtime.runtime_id":"replacement"},
        ),
        (
            "sandboxes",
            doc! {"_id":f.binding.sandbox_id.as_str()},
            doc! {"record.status":"paused"},
        ),
        (
            "sandbox_credentials",
            doc! {"sandbox_id":f.binding.sandbox_id.as_str()},
            doc! {"owner":"other-owner"},
        ),
    ] {
        let collection = db.collection::<Document>(collection);
        let original = collection.find_one(filter.clone()).await.unwrap().unwrap();
        collection
            .update_one(filter.clone(), doc! {"$set":mutation})
            .await
            .unwrap();
        assert!(f.source.authenticate_relay(&bearer, false).await.is_err());
        collection.replace_one(filter, original).await.unwrap();
        f.source.authenticate_relay(&bearer, false).await.unwrap();
    }
    // Settings are mutable; installing the scope does not freeze them forever.
    let mut settings = f
        .store
        .get_agent(&f.conversation.agent_id)
        .await
        .unwrap()
        .unwrap()
        .settings();
    settings.model = "changed-model".into();
    f.store.agents.update_one(doc!{"_id":f.conversation.agent_id.as_str()},
        doc!{"$set":{"record.title":"new title","record.conversation.settings":record_bson(&settings).unwrap()}}).await.unwrap();
    f.source.authenticate_relay(&bearer, false).await.unwrap();
    let custody = db.collection::<Document>("sandbox_credentials");
    let stored = custody
        .find_one(doc! {"sandbox_id":f.binding.sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{stored:?}").contains(ISSUER));
    custody
        .delete_one(doc! {"_id":stored.get_str("_id").unwrap()})
        .await
        .unwrap();
    assert!(f.source.authenticate_relay(&bearer, true).await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_relay_retained_migration_is_idle_fenced_and_preserves_receipt() {
    let f = Fixture::new().await;
    let legacy = MongoProviderToolSource::new(f.store.clone());
    let original = f.open(&legacy).await;
    legacy.confirm_open(&original, NATIVE).await.unwrap();
    let retained = f.open(&f.source).await;
    assert!(retained.transport_origin().is_none());
    let before = f
        .store
        .bindings
        .tool_opens
        .find_one(doc! {"_id":retained.provenance_sha256()})
        .await
        .unwrap()
        .unwrap();
    let agent = f.publish().await;
    assert!(f
        .source
        .prepare_transport(&retained, ORIGIN, NATIVE, 1)
        .await
        .is_err());
    f.idle_intent(agent).await;
    for (origin, native, seq) in [
        ("https://other.example/", NATIVE, 1),
        (ORIGIN, "other-native", 1),
        (ORIGIN, NATIVE, 2),
    ] {
        assert!(f
            .source
            .prepare_transport(&retained, origin, native, seq)
            .await
            .is_err());
    }
    let migrated = f
        .source
        .prepare_transport(&retained, ORIGIN, NATIVE, 1)
        .await
        .unwrap();
    let bearer = f.capability(&migrated);
    f.source.authenticate_relay(&bearer, true).await.unwrap();
    assert!(f.source.authenticate_relay(&bearer, false).await.is_err());
    // A later worker, changed credential generation or queued work invalidates
    // the original observation and prevents a stale maintenance confirmation.
    for (collection, filter, mutation) in [
        (
            "agents",
            doc! {"_id":f.conversation.agent_id.as_str()},
            doc! {"record.work.next_fence":3_i64},
        ),
        (
            "agents",
            doc! {"_id":f.conversation.agent_id.as_str()},
            doc! {"record.work.step":"publish_connection"},
        ),
        (
            "sandboxes",
            doc! {"_id":f.binding.sandbox_id.as_str()},
            doc! {"record.credential.current.generation":2_i64},
        ),
    ] {
        let collection = f
            .client
            .database(&f.database)
            .collection::<Document>(collection);
        let original = collection.find_one(filter.clone()).await.unwrap().unwrap();
        collection
            .update_one(filter.clone(), doc! {"$set":mutation})
            .await
            .unwrap();
        assert!(f
            .source
            .confirm_transport(&migrated, NATIVE, 1)
            .await
            .is_err());
        collection.replace_one(filter, original).await.unwrap();
    }
    f.source
        .confirm_transport(&migrated, NATIVE, 1)
        .await
        .unwrap();
    f.source
        .confirm_transport(&migrated, NATIVE, 1)
        .await
        .unwrap();
    let authenticated = f.source.authenticate_relay(&bearer, false).await.unwrap();
    assert_eq!(authenticated.credential.expose(), ISSUER);
    let after = f
        .store
        .bindings
        .tool_opens
        .find_one(doc! {"_id":retained.provenance_sha256()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    f.client.database(&f.database).drop().await.unwrap();
}

struct ErasingOwner;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ErasingOwner {
    async fn erasing(
        &self,
        _: &layer2_core::SubjectId,
    ) -> std::result::Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn provider_tool_relay_standing_gate_denies_deleting_owner() {
    let mut fixture = Fixture::new().await;
    let attachment = fixture.open(&fixture.source).await;
    let capability = fixture.capability(&attachment);
    fixture
        .source
        .authenticate_relay(&capability, true)
        .await
        .unwrap();
    Arc::get_mut(&mut fixture.source).unwrap().standing = Arc::new(ErasingOwner);
    assert!(fixture
        .source
        .authenticate_relay(&capability, true)
        .await
        .is_err());
    assert!(fixture
        .source
        .authenticate_relay(&capability, false)
        .await
        .is_err());
    fixture
        .client
        .database(&fixture.database)
        .drop()
        .await
        .unwrap();
}
