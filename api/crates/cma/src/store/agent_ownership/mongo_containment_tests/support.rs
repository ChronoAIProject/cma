use super::*;
use mongodb::options::ClientOptions;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize},
    Mutex,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Fault {
    KnownAbort,
    UnknownCommit,
    WithheldReply,
}

// Inject only at one Agent's actual Mongo client. Membership and healthy
// renewals still use the real store, through an unaffected connection pool.
pub(super) struct RoutedStore {
    inner: Arc<MongoAgentOwnership>,
    fault_client: MongoAgentOwnership,
    pub(super) calls: AtomicUsize,
    pub(super) returned: Mutex<Option<OwnershipError>>,
    withhold_reply: AtomicBool,
}

#[async_trait::async_trait]
impl OwnershipStore for RoutedStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.inner.heartbeat(id, draining).await
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.inner.membership().await
    }
    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.owner(id).await
    }
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.acquire(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        if token.agent_id.as_str() != "isolated-agent" {
            return self.inner.renew(token).await;
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = self.fault_client.renew(token).await;
        if self.withhold_reply.load(Ordering::SeqCst) {
            result
                .as_ref()
                .expect("withhold only a committed real Mongo renewal");
            // Lose the successful reply at the store/coordinator boundary.
            // Mongo has committed; the coordinator learns only its own timeout.
            std::future::pending::<()>().await;
        }
        *self.returned.lock().unwrap() = result.as_ref().err().cloned();
        result
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

pub(super) struct Fixture {
    pub(super) store: Arc<MongoAgentOwnership>,
    pub(super) router: Arc<RoutedStore>,
    pub(super) manager: Arc<OwnershipCoordinator>,
    pub(super) faulted: Arc<Authority>,
    pub(super) healthy: Arc<Authority>,
    app: String,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("use disposable Mongo");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_renewal_containment_{}", uuid::Uuid::new_v4().simple());
        let app = format!("renewal-containment-{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
        store
            .database
            .create_collection("test_controls")
            .await
            .unwrap();
        let mut options = ClientOptions::parse(uri).await.unwrap();
        options.app_name = Some(app.clone());
        let fault_client = Client::with_options(options).unwrap();
        let router = Arc::new(RoutedStore {
            inner: store.clone(),
            fault_client: MongoAgentOwnership::open(&fault_client, &name)
                .await
                .unwrap(),
            calls: AtomicUsize::new(0),
            returned: Mutex::new(None),
            withhold_reply: AtomicBool::new(false),
        });
        let manager = OwnershipCoordinator::register(router.clone(), member("containment-member"))
            .await
            .unwrap();
        let faulted = manager
            .acquire(&AgentId("isolated-agent".into()))
            .await
            .unwrap()
            .unwrap();
        let healthy = manager
            .acquire(&AgentId("healthy-agent".into()))
            .await
            .unwrap()
            .unwrap();
        Self {
            store,
            router,
            manager,
            faulted,
            healthy,
            app,
        }
    }

    pub(super) async fn arm(&self, fault: Fault) {
        if matches!(fault, Fault::WithheldReply) {
            self.router.withhold_reply.store(true, Ordering::SeqCst);
            return;
        }
        let (mode, data) = match fault {
            Fault::KnownAbort => (
                mongodb::bson::Bson::String("alwaysOn".into()),
                doc! {"failCommands": ["findAndModify"], "appName": &self.app,
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
            ),
            Fault::UnknownCommit => (
                mongodb::bson::Bson::Document(doc! {"times": 5_i32}),
                doc! {"failCommands": ["commitTransaction"], "appName": &self.app,
                "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected uncertain renewal"}},
            ),
            Fault::WithheldReply => unreachable!("reply is withheld at the store boundary"),
        };
        self.store
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": mode, "data": data,
            })
            .await
            .unwrap();
    }

    pub(super) async fn disarm(&self) {
        self.store
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable injected fault before assertions");
    }

    pub(super) async fn lease(&self, authority: &Authority) -> OwnershipLease {
        self.store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap()
    }

    pub(super) async fn healthy_effect(&self, id: &str) {
        self.healthy
            .run(async {
                self.store.check(&self.healthy.token).await.unwrap();
                let mut session = self.store.client.start_session().await.unwrap();
                start_transaction(&mut session).await.unwrap();
                self.store
                    .fence_transaction(&self.healthy.token, &mut session)
                    .await
                    .unwrap();
                self.store
                    .database
                    .collection::<Document>("test_controls")
                    .insert_one(doc! {"_id": id})
                    .session(&mut session)
                    .await
                    .unwrap();
                assert!(matches!(
                    commit_transaction(&mut session).await,
                    CommitOutcome::Committed
                ));
                assert!(self
                    .store
                    .database
                    .collection::<Document>("test_controls")
                    .find_one(doc! {"_id": id})
                    .await
                    .unwrap()
                    .is_some());
            })
            .await
            .unwrap();
    }

    pub(super) async fn close(&self) {
        self.manager.begin_drain().await;
        self.store.database.drop().await.unwrap();
    }
}
