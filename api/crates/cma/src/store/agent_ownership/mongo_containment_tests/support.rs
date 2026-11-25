use super::*;
use crate::store::agent_ownership::mongo_renewal_faults::{Fault, RoutedStore};
use layer3_adapter::owner_protocol::ReceiverRevocation;
use std::sync::atomic::{AtomicBool, AtomicUsize};

struct Receiver {
    acknowledges: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "containment-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.acknowledges.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic receiver refusal".into(),
            ))
        }
    }
}

pub(super) struct Fixture {
    pub(super) store: Arc<MongoAgentOwnership>,
    pub(super) router: Arc<RoutedStore>,
    pub(super) manager: Arc<OwnershipCoordinator>,
    pub(super) faulted: Arc<Authority>,
    pub(super) healthy: Arc<Authority>,
    receiver: Arc<Receiver>,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        Self::with_policy(layer2_core::agent_ownership::MaintenancePolicy::Legacy).await
    }

    pub(super) async fn with_policy(
        policy: layer2_core::agent_ownership::MaintenancePolicy,
    ) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("use disposable Mongo");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_renewal_containment_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
        store
            .database
            .create_collection("test_controls")
            .await
            .unwrap();
        let router = RoutedStore::new(store.clone(), AgentId("isolated-agent".into())).await;
        let manager = OwnershipCoordinator::register_with_policy(
            router.clone(),
            member("containment-member"),
            false,
            policy,
        )
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
        let receiver = Arc::new(Receiver {
            acknowledges: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        faulted.register_receiver(receiver.clone()).unwrap();
        Self {
            receiver,
            store,
            router,
            manager,
            faulted,
            healthy,
        }
    }

    pub(super) async fn arm(&self, fault: Fault) {
        self.router.arm(fault).await;
    }
    pub(super) async fn disarm(&self) {
        self.router.disarm().await;
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

    pub(super) async fn verify_cleanup(&self, retained: &OwnershipLease, fault: Fault) {
        assert!(self
            .faulted
            .register_receiver(self.receiver.clone())
            .is_err());
        assert!(self
            .manager
            .handoff(&self.faulted, self.faulted.revoke_transports())
            .await
            .is_err());
        let observed = self.lease(&self.faulted).await;
        assert_eq!(observed.token, retained.token);
        if matches!(fault, Fault::UnknownCommit) {
            assert!(observed.expires_at_ms >= retained.expires_at_ms);
        } else {
            assert_eq!(observed, *retained, "refused confirmation shortened lease");
        }
        assert_eq!(self.router.releases.load(Ordering::SeqCst), 0);
        assert!(self
            .manager
            .cancelled_receivers()
            .await
            .iter()
            .any(|a| Arc::ptr_eq(a, &self.faulted)));
        self.manager.maintain().await.unwrap();
        self.healthy_effect("after-refused-cleanup").await;
        assert!(!*self.manager.failure_signal().borrow());
        self.receiver.acknowledges.store(true, Ordering::SeqCst);
        self.manager
            .handoff(&self.faulted, self.faulted.revoke_transports())
            .await
            .unwrap();
        assert_eq!(self.receiver.calls.load(Ordering::SeqCst), 2);
        assert!(self
            .store
            .owner(&self.faulted.token.agent_id)
            .await
            .unwrap()
            .is_none());
        let next = self
            .manager
            .acquire(&self.faulted.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert!(next.token.epoch > self.faulted.token.epoch);
        assert!(!Arc::ptr_eq(&next, &self.faulted));
        self.manager
            .handoff(&self.faulted, self.faulted.revoke_transports())
            .await
            .unwrap();
        assert_eq!(self.lease(&next).await.token, next.token);
        assert!(next.is_current());
        assert_eq!(self.router.releases.load(Ordering::SeqCst), 1);
        assert!(self
            .faulted
            .run(async { panic!("old token revived") })
            .await
            .is_err());
        assert!(Arc::ptr_eq(
            &self.manager.local(&next.token.agent_id).await.unwrap(),
            &next
        ));
    }

    #[cfg(feature = "qualification-hooks")]
    pub(super) fn acknowledge_receiver(&self) {
        self.receiver.acknowledges.store(true, Ordering::SeqCst);
    }

    pub(super) async fn close(&self) {
        self.manager.begin_drain().await;
        self.store.database.drop().await.unwrap();
    }
}
