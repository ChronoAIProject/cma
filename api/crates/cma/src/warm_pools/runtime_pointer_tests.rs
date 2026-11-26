use super::{resize, target, NativePoolControl};
use crate::{
    store::native_warm_pools::MongoNativeWarmPools,
    warm_pools::{
        deployment_image, NativePoolBinding, NativePoolConfiguration, NativePoolSpec,
        NativePoolStatus, NativeWarmPools, PoolTarget, PoolTargetStore,
    },
};
use async_trait::async_trait;
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    SandboxCapacity,
};
use mongodb::{options::ClientOptions, Client, Database};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[path = "runtime_pointer_tests/provider.rs"]
mod provider;
use provider::{Effect, Provider};

#[path = "runtime_pointer_tests/lifecycle_backend.rs"]
mod lifecycle_backend;
use lifecycle_backend::HostedFixture;

#[path = "runtime_pointer_tests/promotion.rs"]
mod promotion;
#[path = "runtime_pointer_tests/retirement_store.rs"]
mod retirement_store;
use retirement_store::RetirementStore;

const DEADLINE: Duration = Duration::from_secs(10);
const DEPLOYMENT: &str = "test";
const TARGET: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Lane {
    Agent,
    Machine,
}

fn image(lane: Lane, release: char) -> String {
    let repository = match lane {
        Lane::Agent => "agent",
        Lane::Machine => "machine",
    };
    resize::pinned(repository, release)
}

fn capacity() -> SandboxCapacity {
    SandboxCapacity {
        cpu_millis: 1000,
        memory_mib: 1024,
    }
}

fn home_policy() -> layer3_adapter::HomeVolumePolicy {
    layer3_adapter::HomeVolumePolicy {
        mode: layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "fixture".into(),
        capacity_gib: 20,
        template_name: String::new(),
        template_generation: String::new(),
    }
}

struct Replica {
    database: Database,
    store: Arc<MongoNativeWarmPools>,
    retirement: Arc<RetirementStore>,
    pools: Arc<NativeWarmPools>,
    provider: Provider,
    mongo_app_name: String,
    hosted: HostedFixture,
}

impl Replica {
    async fn seed_defaults(&self) {
        self.pools.ensure_default().await.unwrap();
    }

    async fn boot(&self) {
        self.seed_defaults().await;
        tokio::time::timeout(DEADLINE, self.pools.await_default_ready())
            .await
            .expect("Agent Manager startup readiness must settle")
            .unwrap();
    }

    async fn target_for(&self, plan: &NativePoolConfiguration) -> PoolTarget {
        let rows = self.store.list().await.unwrap();
        let matching = rows.into_iter().filter(|row| &row.plan == plan);
        resize::only(matching.collect())
    }
}

struct Fixture {
    mongo: ClientOptions,
    database: Database,
    provider: Provider,
    hosted: HostedFixture,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("disposable local Mongo URI is required");
        let mut mongo = ClientOptions::parse(uri).await.unwrap();
        assert!(mongo.credential.is_none());
        assert!(mongo.repl_set_name.is_some());
        assert!(mongo.hosts.iter().all(|host| matches!(
            host,
            mongodb::options::ServerAddress::Tcp { host, .. }
                if ["127.0.0.1", "localhost", "::1"].contains(&host.as_str())
        )));
        mongo.server_selection_timeout = Some(DEADLINE);
        let client = Client::with_options(mongo.clone()).unwrap();
        let database = client.database(&format!(
            "cma_runtime_pointer_{}",
            uuid::Uuid::new_v4().simple()
        ));
        Self {
            mongo,
            database,
            provider: Provider::default(),
            hosted: Default::default(),
        }
    }

    // Independent process-local adapter, controller and Mongo client. Restart
    // drops those objects and opens the same database and provider again.
    async fn replica(&self) -> Replica {
        self.replica_pinned_to('a').await
    }

    async fn replica_pinned_to(&self, release: char) -> Replica {
        let mut options = self.mongo.clone();
        let mongo_app_name = format!("pointer-replica-{}", uuid::Uuid::new_v4().simple());
        options.app_name = Some(mongo_app_name.clone());
        let client = Client::with_options(options).unwrap();
        let database = client.database(self.database.name());
        let store = Arc::new(
            crate::store::open_pointer_fixture_pool_store(&database, DEPLOYMENT)
                .await
                .unwrap(),
        );
        let provider = self.provider.for_replica();
        let backend = resize::PoolBackend(provider.clone());
        let adapter = CodexAdapter::with_backend(backend, CodexSandboxSpec::default());
        let retirement = Arc::new(RetirementStore::new(store.clone()));
        let pools = NativeWarmPools::new(
            retirement.clone(),
            Arc::new(adapter),
            image(Lane::Agent, release),
            Some(home_policy()),
            capacity(),
            TARGET,
            Some((image(Lane::Machine, '1'), TARGET)),
        )
        .with_target_fencing(true);
        Replica {
            database,
            store,
            retirement,
            pools,
            provider,
            mongo_app_name,
            hosted: self.hosted.clone(),
        }
    }

    async fn restart(&self, original: Replica) -> Replica {
        drop(original);
        self.replica().await
    }

    fn plan(&self, lane: Lane, release: char, size: SandboxCapacity) -> NativePoolConfiguration {
        self.provider
            .plan(&NativePoolSpec {
                cold_canary: false,
                image: deployment_image(image(lane, release)),
                cpu_millis: size.cpu_millis,
                memory_mib: size.memory_mib,
                home: home_policy(),
            })
            .unwrap()
    }

    fn readiness(&self, plan: &NativePoolConfiguration, ready: u32) {
        self.provider
            .inventory
            .lock()
            .unwrap()
            .readiness(plan, ready);
    }
}

#[path = "runtime_pointer_tests/mongo.rs"]
mod mongo;

#[path = "runtime_pointer_tests/fixed_startup.rs"]
mod fixed_startup;

#[path = "runtime_pointer_tests/released_source.rs"]
pub(crate) mod released_source;
#[path = "runtime_pointer_tests/shared_consumer_tests.rs"]
mod shared_consumer_tests;
