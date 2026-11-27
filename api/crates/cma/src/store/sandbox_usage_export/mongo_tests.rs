use super::*;
use crate::store::sandbox_mongo_integration_tests::sandbox;
use layer2_core::{
    sandbox::store::{SandboxEventStore, SandboxStore},
    sandbox::{RuntimeBinding, Sandbox, SandboxEventKind},
    sandbox_usage_service::SandboxUsageStore,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

mod capture;
mod completeness;
mod delivery;
mod delivery_contract;
mod delivery_recovery;
mod delivery_worker;
mod erasure;
mod inventory;
mod projection;
mod safety;

const NOW: &str = "2026-09-05T00:00:00Z";
const MINUTE: &str = "2026-09-05T00:01:00Z";
const TWO_MINUTES: &str = "2026-09-05T00:02:00Z";

struct Fixture {
    client: Client,
    db: Database,
    store: MongoSandboxStore,
    commands: Arc<Mutex<Vec<Document>>>,
}

impl Fixture {
    async fn new(enabled: bool) -> Self {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let trace = commands.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                trace.lock().unwrap().push(event.command);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let db = client.database(&format!(
            "cma_usage_capture_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = MongoSandboxStore::open(&client, db.name()).await.unwrap();
        let mut fixture = Self {
            client,
            db,
            store,
            commands,
        };
        if enabled {
            fixture.prepare().await;
            fixture.store = fixture
                .store
                .clone()
                .with_usage_capture(Some(config()))
                .await
                .unwrap();
        }
        fixture
    }

    async fn prepare(&self) {
        for _ in 0..schema::MAX_STEPS {
            if !schema::apply_next(&self.db).await.unwrap() {
                break;
            }
        }
        assert_eq!(schema::pending(&self.db).await.unwrap(), 0);
    }

    async fn parent(&self, owner: &str) -> Sandbox {
        self.parent_with_generation(owner, Some(1)).await
    }

    async fn parent_with_generation(&self, owner: &str, generation: Option<u64>) -> Sandbox {
        let mut parent = sandbox(&layer2_core::SandboxId::generate().to_string());
        parent.owner = layer2_core::SubjectId::new(owner);
        parent.name = format!("fixture-{}", uuid::Uuid::new_v4().simple());
        parent.runtime = Some(RuntimeBinding {
            runtime_id: "runtime-fixture".into(),
            runtime_version: "image-fixture".into(),
            bound_at: NOW.into(),
            boot_id: None,
            provision_generation: generation,
        });
        parent.lifecycle.runtime_generation_high_water = generation.unwrap_or(0);
        self.store.create_sandbox(parent.clone()).await.unwrap();
        self.store
            .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
            .await
            .unwrap();
        parent
    }

    fn capture(&self) -> CaptureStore {
        self.store.usage_capture.clone().unwrap()
    }
    async fn count(&self, collection: &str) -> u64 {
        self.db
            .collection::<Document>(collection)
            .count_documents(doc! {})
            .await
            .unwrap()
    }

    async fn wire_events(&self) -> Vec<serde_json::Value> {
        use futures_util::TryStreamExt;
        let rows: Vec<Document> = self
            .db
            .collection(OUTBOX)
            .find(doc! {})
            .sort(doc! {"sequence":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert!(!rows.is_empty());
        rows.iter()
            .map(|row| {
                let bytes = row.get_binary_generic("payload").unwrap();
                let value = serde_json::from_slice(bytes).unwrap();
                super::fixture_tests::assert_wire_event(&value);
                assert_eq!(serde_json_canonicalizer::to_vec(&value).unwrap(), *bytes);
                assert_eq!(records::hash(bytes), row.get_str("digest").unwrap());
                value
            })
            .collect()
    }
}

fn config() -> UsageCaptureConfig {
    UsageCaptureConfig {
        completeness_enabled: false,
        namespace: "cma-fixture".into(),
        owner_issuer: "https://identity.example".into(),
    }
}

fn source_id(parent: &Sandbox, revision: u64) -> String {
    format!("cma-usage:{}:{revision}", parent.id)
}

fn now() -> u64 {
    layer2_core::sandbox_usage::timestamp("2026-09-10T00:00:00Z").unwrap()
}
