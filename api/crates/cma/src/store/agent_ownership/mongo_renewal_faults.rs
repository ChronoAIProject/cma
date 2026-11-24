//! Targeted real-Mongo faults shared by custody and public-route regressions.
use super::*;
use layer2_core::agent_ownership::RenewalBatch;
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
    reply_waiting: tokio::sync::Notify,
    reply_allowed: tokio::sync::Notify,
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
            self.reply_waiting.notify_one();
            self.reply_allowed.notified().await;
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
            reply_waiting: tokio::sync::Notify::new(),
            reply_allowed: tokio::sync::Notify::new(),
        })
    }

    #[cfg(feature = "qualification-hooks")]
    pub(crate) async fn wait_for_held_reply(&self) {
        self.reply_waiting.notified().await;
    }

    #[cfg(feature = "qualification-hooks")]
    pub(crate) fn allow_held_reply(&self) {
        self.reply_allowed.notify_one();
    }
    pub(crate) async fn arm(&self, fault: Fault) {
        if matches!(fault, Fault::WithheldReply) {
            self.withhold_reply.store(true, Ordering::SeqCst);
            return;
        }
        let (mode, data) = match fault {
            Fault::KnownAbort => (
                mongodb::bson::Bson::String("alwaysOn".into()),
                // Reject the first transactional command before MongoDB opens
                // the transaction. Failing the later owner write can strand
                // the shared member write if the retry deadline cuts off abort.
                doc! {"failCommands": ["find"], "appName": &self.app,
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
            ),
            Fault::UnknownCommit => (
                mongodb::bson::Bson::Document(doc! {"times": 5_i32}),
                doc! {"failCommands": ["commitTransaction"], "appName": &self.app,
                "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected uncertain renewal"}},
            ),
            Fault::WithheldReply => unreachable!("reply is withheld at the store boundary"),
        };
        self.inner
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": mode, "data": data,
            })
            .await
            .unwrap();
    }

    pub(crate) async fn disarm(&self) {
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
