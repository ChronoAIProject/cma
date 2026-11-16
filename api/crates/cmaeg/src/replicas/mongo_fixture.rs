//! Independent authenticated clients; no shared process-local ownership registry.
use super::{
    drain::Control,
    membership::{Membership, Worker},
    partition::Partitions,
    store::Store,
};
use crate::{
    http::readiness::Readiness,
    records::{
        mongo::{self, MongoRecords},
        RecordClient,
    },
    turns::TurnTracker,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::seal::MasterKeyRing;
use mongodb::{
    bson::doc,
    event::{command::CommandEvent, EventHandler},
    options::{ClientOptions, Credential},
    Client,
};
use ring::rand::SecureRandom;
use std::sync::Arc;

pub(crate) struct Fixture {
    admin: Client,
    database: String,
    uri: String,
    password: String,
    keys: Arc<MasterKeyRing>,
}
pub(crate) struct Node {
    pub partitions: Partitions,
    pub worker: Option<Worker>,
    pub control: Control,
}
impl Node {
    pub async fn claim_effect(
        &self,
        plan: &crate::ingress::routing_store::AdmittedPlan,
        target: &crate::ingress::routing_store::StoredTarget,
        work_id: &str,
    ) -> Result<super::partition::PartitionLease, super::Error> {
        self.partitions.claim_work(plan, target, work_id).await
    }
    pub fn journal(&self) -> crate::turns::routed::Journal {
        crate::turns::routed::Journal::new(
            self.partitions.store.records.clone(),
            self.partitions.store.keys.clone(),
        )
        .with_partitions(self.partitions.clone())
    }
    pub fn plans(&self) -> crate::ingress::routing_store::PlanStore {
        crate::ingress::routing_store::PlanStore::new(
            self.partitions.store.records.clone(),
            self.partitions.store.keys.clone(),
        )
        .with_admission_queue(Arc::new(self.partitions.clone()))
    }
}
impl Fixture {
    pub fn keys(&self) -> Arc<MasterKeyRing> {
        self.keys.clone()
    }

    pub async fn enable_media(&self) {
        let db = self.admin.database(&self.database);
        db.run_command(mongo::media_continuations_role(&self.database, false))
            .await
            .unwrap();
        mongo::migrate_media_continuations(&db).await.unwrap();
        db.run_command(doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_media_continuations_runtime","db":&self.database}]}).await.unwrap();
    }

    pub async fn new() -> Self {
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI")
            .unwrap_or_else(|_| panic!("authenticated disposable Mongo required"));
        let admin = Client::with_uri_str(&uri)
            .await
            .unwrap_or_else(|_| panic!("fixture client failed"));
        let database = format!("cmaeg_replica_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&database);
        for migration in [false, true] {
            db.run_command(mongo::role(&database, migration))
                .await
                .unwrap_or_else(|_| panic!("role creation failed"));
        }
        let password = uuid::Uuid::new_v4().to_string();
        for (user, role) in [
            ("runtime", "cma_eg_runtime"),
            ("migrator", "cma_eg_migrator"),
        ] {
            db.run_command(
                doc! {"createUser":user,"pwd":&password,"roles":[{"role":role,"db":&database}]},
            )
            .await
            .unwrap_or_else(|_| panic!("fixture user creation failed"));
        }
        let mut key = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut key)
            .unwrap_or_else(|_| panic!("randomness unavailable"));
        let fixture = Self {
            admin,
            database,
            uri,
            password,
            keys: Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode(key))).unwrap()),
        };
        let migrator = fixture.client("migrator").await;
        mongo::migrate_replicas(&migrator.database(&fixture.database))
            .await
            .unwrap_or_else(|_| panic!("replica migration failed"));
        fixture
    }
    async fn client(&self, user: &str) -> Client {
        self.client_with_handler(user, None).await
    }
    async fn client_with_handler(
        &self,
        user: &str,
        handler: Option<EventHandler<CommandEvent>>,
    ) -> Client {
        let mut options = ClientOptions::parse(&self.uri)
            .await
            .unwrap_or_else(|_| panic!("fixture options failed"));
        options.credential = Some(
            Credential::builder()
                .username(user.to_owned())
                .password(self.password.clone())
                .source(self.database.clone())
                .build(),
        );
        options.max_pool_size = Some(4);
        options.command_event_handler = handler;
        Client::with_options(options)
            .unwrap_or_else(|_| panic!("fixture client configuration failed"))
    }
    pub async fn node(&self, index: u16) -> Node {
        self.node_with_handler(index, None).await
    }
    pub async fn node_with_handler(
        &self,
        index: u16,
        handler: Option<EventHandler<CommandEvent>>,
    ) -> Node {
        let client = self.client_with_handler("runtime", handler).await;
        let mongo = MongoRecords::open(client.database(&self.database))
            .await
            .unwrap_or_else(|_| panic!("runtime grant check failed"));
        let records = RecordClient::new("http://127.0.0.1:1", uuid::Uuid::new_v4().to_string())
            .unwrap()
            .with_database(mongo);
        let store = Store::new(records, self.keys.clone()).unwrap();
        let readiness = Readiness::initialized();
        let port = 20000 + index;
        let control = Control::new(
            self.keys.clone(),
            TurnTracker::new(readiness.clone()),
            port,
            "https://replica-fixture.invalid",
        );
        let (membership, worker) = Membership::join(
            store.clone(),
            "https://replica-fixture.invalid",
            readiness,
            format!("http://127.0.0.1:{port}"),
            control.clone(),
        )
        .await
        .unwrap();
        Node {
            partitions: Partitions::new(store, membership),
            worker: Some(worker),
            control,
        }
    }
    pub async fn cleanup(self) {
        self.admin
            .database(&self.database)
            .drop()
            .await
            .unwrap_or_else(|_| panic!("fixture cleanup failed"));
    }
}

impl Node {
    /// Fault injection: revoke this client's exact live resource claim, without
    /// changing clocks/constants or installing a successor's authority locally.
    pub async fn lose_work(&self, work_id: &str) -> Result<(), super::Error> {
        let token = self
            .partitions
            .local
            .read()
            .unwrap()
            .values()
            .find(|token| token.work_id == work_id)
            .cloned()
            .ok_or(super::Error::Lost)?;
        self.partitions.release(&token).await
    }
}
