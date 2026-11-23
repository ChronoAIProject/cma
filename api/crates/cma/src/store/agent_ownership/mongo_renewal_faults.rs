//! Targeted real-Mongo faults shared by custody and public-route regressions.
use super::*;
use layer2_core::agent_ownership::RenewalBatch;
use mongodb::error::{ErrorKind, TRANSIENT_TRANSACTION_ERROR};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Fault {
    KnownAbort,
    UnknownCommit,
    WithheldReply,
}

// Inject only at one Agent's actual Mongo client. Membership and healthy
// renewals still use the real store, through an unaffected connection pool.
pub(crate) struct RoutedStore {
    inner: Arc<MongoAgentOwnership>,
    target: AgentId,
    app: String,
    fault_client: MongoAgentOwnership,
    pub(crate) calls: AtomicUsize,
    pub(crate) releases: AtomicUsize,
    pub(crate) returned: Mutex<Option<OwnershipError>>,
    withhold_reply: AtomicBool,
    owner_write: Mutex<Option<ClientSession>>,
    owner_write_conflicts: Arc<AtomicUsize>,
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
        if token.agent_id != self.target {
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
    fn supports_batch_renewal(&self) -> bool {
        true
    }
    async fn renew_batch(&self, tokens: &[OwnershipToken]) -> Result<RenewalBatch, OwnershipError> {
        if !tokens.iter().any(|token| token.agent_id == self.target) {
            return self.inner.renew_batch(tokens).await;
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = self.fault_client.renew_batch(tokens).await;
        if self.withhold_reply.load(Ordering::SeqCst) {
            assert!(matches!(result, Ok(RenewalBatch::Committed(_))));
            std::future::pending::<()>().await;
        }
        result
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        if token.agent_id == self.target {
            self.releases.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

impl RoutedStore {
    pub(crate) async fn new(inner: Arc<MongoAgentOwnership>, target: AgentId) -> Arc<Self> {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("use disposable Mongo");
        let app = format!("renewal-containment-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(uri).await.unwrap();
        options.app_name = Some(app.clone());
        let owner_write_conflicts = Arc::new(AtomicUsize::new(0));
        let conflicts = owner_write_conflicts.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let CommandEvent::Failed(event) = event else {
                return;
            };
            if event.command_name == "findAndModify"
                && event.failure.contains_label(TRANSIENT_TRANSACTION_ERROR)
                && matches!(event.failure.kind.as_ref(), ErrorKind::Command(command) if command.code == 112)
            {
                conflicts.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let client = Client::with_options(options).unwrap();
        Arc::new(Self {
            fault_client: MongoAgentOwnership::open(&client, inner.database.name())
                .await
                .unwrap(),
            inner,
            target,
            app,
            calls: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            returned: Mutex::new(None),
            withhold_reply: AtomicBool::new(false),
            owner_write: Mutex::new(None),
            owner_write_conflicts,
        })
    }
    pub(crate) async fn arm(&self, fault: Fault) {
        match fault {
            Fault::KnownAbort => {
                // A real write conflict rolls back the renewal on the server
                // before it returns code 112. A failCommand response can leave
                // the member write open if the deadline cuts off the abort.
                // Hold only the faulted owner row so healthy writes can proceed.
                let mut session = self.inner.client.start_session().await.unwrap();
                start_transaction(&mut session).await.unwrap();
                let held = self
                    .inner
                    .owners
                    .update_one(
                        doc! {"_id": self.target.as_str()},
                        doc! {"$inc": {"effect_revision": 1_i64}},
                    )
                    .session(&mut session)
                    .await
                    .unwrap();
                assert_eq!(held.modified_count, 1, "hold the faulted owner row");
                self.owner_write_conflicts.store(0, Ordering::SeqCst);
                assert!(self.owner_write.lock().unwrap().replace(session).is_none());
            }
            Fault::UnknownCommit => {
                self.inner
                    .client
                    .database("admin")
                    .run_command(doc! {
                        "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
                        "data": {"failCommands": ["commitTransaction"], "appName": &self.app,
                            "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected uncertain renewal"}},
                    })
                    .await
                    .unwrap();
            }
            Fault::WithheldReply => self.withhold_reply.store(true, Ordering::SeqCst),
        }
    }

    pub(crate) async fn disarm(&self) {
        let owner_write = self.owner_write.lock().unwrap().take();
        if let Some(mut session) = owner_write {
            session.abort_transaction().await.unwrap();
            assert!(
                self.owner_write_conflicts.load(Ordering::SeqCst) >= 2,
                "targeted renewal must retry a server-aborted owner write conflict"
            );
            return;
        }
        if self.withhold_reply.swap(false, Ordering::SeqCst) {
            return;
        }
        self.inner
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable injected fault before assertions");
    }
}
