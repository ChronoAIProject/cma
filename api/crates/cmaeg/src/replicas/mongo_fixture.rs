//! Independent authenticated clients; no shared process-local ownership registry.
use super::{
    drain::Control,
    membership::{Membership, Worker},
    mongo_counter::{self, CommandCounter},
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

#[path = "mongo_faults.rs"]
mod faults;

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
    counter: Option<Arc<CommandCounter>>,
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
        let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI")
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
    pub async fn records(&self, base: &str) -> RecordClient {
        self.records_with_tracking(base, None).await
    }
    pub async fn records_with_tracking(
        &self,
        base: &str,
        tracking: Option<crate::worker_credentials::WorkerCredentials>,
    ) -> RecordClient {
        let client = self.client("runtime").await;
        let mut mongo = MongoRecords::open(client.database(&self.database))
            .await
            .unwrap();
        if let Some(tracking) = tracking {
            mongo.set_worker_tracking(tracking);
        }
        RecordClient::new(base, uuid::Uuid::new_v4().to_string())
            .unwrap()
            .with_database(mongo)
    }
    pub async fn node(&self, index: u16) -> Node {
        self.node_with_handler(index, None).await
    }
    pub async fn node_with_handler(
        &self,
        index: u16,
        handler: Option<EventHandler<CommandEvent>>,
    ) -> Node {
        let counter = handler
            .is_none()
            .then(|| Arc::new(CommandCounter::default()));
        let handler = match counter.clone() {
            Some(counter) => Some(mongo_counter::handler(counter)),
            None => handler,
        };
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
            counter,
        }
    }
    /// Hold expired rows in this disposable database to exercise pre-TTL reads.
    pub async fn hold_expired_interactions(&self) {
        self.admin
            .database(&self.database)
            .collection::<mongodb::bson::Document>(mongo::INTERACTIONS)
            .drop_index("expires_at_1")
            .await
            .unwrap();
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
    pub fn command_counter(&self) -> Option<Arc<CommandCounter>> {
        self.counter.clone()
    }

    pub async fn reset_frontier_snapshot(&self) {
        *self.partitions.snapshot.lock().await = Default::default();
    }

    pub async fn corrupt_lane_capability(&self) {
        let id = crate::connections::store::index_id(
            "replica-delayed-lane-capability-v1",
            &[&self.partitions.membership.instance],
        );
        let (_, version) = self
            .partitions
            .store
            .read::<serde_json::Value>(&id, true)
            .await
            .unwrap()
            .unwrap();
        self.partitions
            .store
            .write(
                &id,
                &self.partitions.membership.deployment,
                &serde_json::json!({"not_a_lane_capability": true}),
                super::store::Write {
                    retained_until: Some(
                        self.partitions.store.clock().await.unwrap().millis / 1000 + 3600,
                    ),
                    expected: Some(version),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

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

impl Node {
    pub async fn hide_capability(&self) {
        let id = crate::connections::store::index_id(
            "replica-member-capability-v1",
            &[&self.partitions.membership.instance],
        );
        self.partitions
            .store
            .records
            .clone()
            .interactions()
            .delete(&id.parse().unwrap(), 1)
            .await
            .unwrap();
    }
}

impl Node {
    pub async fn work_binding(
        &self,
        token: &super::partition::PartitionLease,
    ) -> super::unresolved::Binding {
        let (_, version) = self.partitions.head(&token.id, &token.owner).await.unwrap();
        super::unresolved::Binding {
            partition: token.id.clone(),
            head_work: token.work_id.clone(),
            head_version: version,
        }
    }
}

impl Node {
    pub async fn seed_expired_operator_index(&self, entries: usize) {
        use std::collections::BTreeMap;
        let now = self
            .partitions
            .store
            .clock()
            .await
            .unwrap()
            .millis
            .div_euclid(1000);
        let audits = (0..entries)
            .map(|_| {
                super::unresolved::identifier(
                    "replica-abandonment-v1",
                    &uuid::Uuid::new_v4().to_string(),
                )
            })
            .collect::<Vec<_>>();
        let index = super::unresolved::Index {
            kind: "replica_abandonment_index_v1".into(),
            deployment: self.partitions.membership.deployment.clone(),
            deadlines: audits
                .iter()
                .map(|id| (id.clone(), now - 60))
                .collect::<BTreeMap<_, _>>(),
            audits,
            rollback_safe_after: Some(super::unresolved::utc(now - 60).unwrap()),
        };
        self.partitions
            .store
            .write(
                &super::unresolved::identifier("replica-abandonment-index-v1", &index.deployment),
                &index.deployment,
                &index,
                super::store::Write::default(),
            )
            .await
            .unwrap();
    }
    pub async fn operator_index(&self) -> (super::unresolved::Index, u64) {
        self.partitions
            .store
            .read(
                &super::unresolved::identifier(
                    "replica-abandonment-index-v1",
                    &self.partitions.membership.deployment,
                ),
                false,
            )
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn head_binding(
        &self,
        plan: &crate::ingress::routing_store::AdmittedPlan,
        target: &crate::ingress::routing_store::StoredTarget,
    ) -> super::unresolved::Binding {
        let id = super::partition_model::partition_id(
            &self.partitions.membership.deployment,
            plan,
            target,
        );
        let (head, version) = self
            .partitions
            .head(&id, &plan.identity.owner)
            .await
            .unwrap();
        super::unresolved::Binding {
            partition: id,
            head_work: head.queue.front().unwrap().clone(),
            head_version: version,
        }
    }
}

impl Node {
    pub async fn lane_capability(&self, enabled: bool) {
        let id = crate::connections::store::index_id(
            "replica-delayed-lane-capability-v1",
            &[&self.partitions.membership.instance],
        );
        let records = self.partitions.store.records.clone().interactions();
        if !enabled {
            records.delete(&id.parse().unwrap(), 1).await.unwrap();
        } else {
            self.partitions.store.write(&id, &self.partitions.membership.deployment, &serde_json::json!({"kind":"replica_delayed_lane_capability_v1","deployment":self.partitions.membership.deployment,"instance":self.partitions.membership.instance,"delayed_lane_v1":true}), super::store::Write { retained_until: Some(self.partitions.store.clock().await.unwrap().millis/1000 + 30*86400), ..Default::default() }).await.unwrap();
        }
    }
    pub async fn custody(&self, work: &str) -> Option<(String, bool, i64)> {
        self.partitions
            .store
            .read::<super::partition_model::Item>(&super::partition_model::item_id(work), true)
            .await
            .unwrap()
            .map(|(item, _)| (item.partition, item.done, item.retained_until))
    }
    pub async fn expire_lane(&self, id: &str, work: &str) {
        use mongodb::bson::{doc, DateTime};
        let store = &self.partitions.store;
        let now = store.clock().await.unwrap().millis;
        let collection = store.collection(true).unwrap();
        for id in [id.to_owned(), super::partition_model::item_id(work)] {
            collection
                .update_one(
                    doc! {"_id":id},
                    doc! {"$set":{"expires_at":DateTime::from_millis(now-1000)}},
                )
                .await
                .unwrap();
        }
    }
    pub async fn storage_size(&self) -> (usize, usize) {
        use futures_util::TryStreamExt;
        let rows: Vec<mongodb::bson::Document> = self
            .partitions
            .store
            .collection(true)
            .unwrap()
            .find(mongodb::bson::doc! {})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        (
            rows.len(),
            rows.iter()
                .map(|row| mongodb::bson::serialize_to_vec(row).unwrap().len())
                .sum(),
        )
    }
}

impl Node {
    pub async fn legacy_partition_work(&self) -> Vec<String> {
        self.partitions
            .store
            .active::<super::partition_model::Head>(
                crate::records::mongo::ReplicaKind::Partition,
                &self.partitions.membership.deployment,
                None,
                128,
            )
            .await
            .unwrap()
            .into_iter()
            .flat_map(|(_, head, _)| head.queue)
            .collect()
    }
}
