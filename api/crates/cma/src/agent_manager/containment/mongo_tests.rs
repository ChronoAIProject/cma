#[path = "composition_tests.rs"]
mod composition;

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use layer3_adapter::owner_protocol::{
    ReceiverCleanup, ReceiverConfirmationCustody, ReceiverRevocation, ReceiverSettlementFailure,
    ReceiverSettlementWatch, TransportAuthority,
};
use std::sync::atomic::AtomicUsize;

struct Receiver {
    acknowledges: AtomicBool,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "housekeeping-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        panic!("contained manager must use typed settlement")
    }
    async fn settle_typed(
        &self,
        _: ReceiverConfirmationCustody,
        _: &ReceiverSettlementWatch,
    ) -> Result<ReceiverCleanup, ReceiverSettlementFailure> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if self.acknowledges.load(Ordering::Acquire) {
            Ok(ReceiverCleanup::Acknowledged)
        } else {
            Err(ReceiverSettlementFailure::Unavailable)
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn actual_housekeeping_recovers_inactive_and_cooperative_retirements_without_new_hints() {
    for (pacing, inactive) in [false, true]
        .into_iter()
        .flat_map(|pacing| [false, true].map(|inactive| (pacing, inactive)))
    {
        let (client, name, store) = database().await;
        let (_, healthy_agent) = fixture(&store, "healthy-housekeeping").await;
        let old_id = if inactive {
            AgentId("absent-inactive-agent".into())
        } else {
            fixture(&store, "cooperative-housekeeping").await.1.id
        };
        let store = Arc::new(
            store
                .enable_agent_ownership("housekeeping-member".into())
                .await
                .unwrap(),
        );
        let manager = AgentManager::register_with_policy(
            store,
            Arc::new(LiveAgentRegistry::default()),
            "http://manager.invalid".into(),
            2,
            pacing,
            MaintenancePolicy::ContainFailures,
        )
        .await
        .unwrap();
        manager.enable_receiver_containment();
        manager
            .register_lifecycle(crate::agent_lifecycle::LifecycleDrain::fixture(1))
            .unwrap();
        manager.activate().await.unwrap();
        let old = manager.coordinator.acquire(&old_id).await.unwrap().unwrap();
        let healthy = manager
            .coordinator
            .acquire(&healthy_agent.id)
            .await
            .unwrap()
            .unwrap();
        let receiver = Arc::new(Receiver {
            acknowledges: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        old.register_receiver(receiver.clone()).unwrap();
        old.begin_receiver_confirmation(&receiver.identity())
            .unwrap();
        old.receiver_confirmed(&receiver.identity());
        if inactive {
            // The clock-driven scheduling unit tests cover the full idle wait;
            // seed its elapsed deadline here while Mongo keeps real server time.
            manager.containment_times.lock().unwrap().entries.insert(
                old_id.clone(),
                Timers {
                    token: old.token.clone(),
                    cooperative: None,
                    inactive: Some(Instant::now() - Duration::from_secs(61)),
                },
            );
        }
        manager
            .service_retirements(if inactive { vec![] } else { vec![old.clone()] })
            .await
            .unwrap();
        manager.service_retirements(vec![]).await.unwrap();
        assert!(!old.is_current());
        assert_eq!(manager.contained.pending(), 0);
        assert!(manager.is_ready() && healthy.is_current());
        assert_eq!(receiver.calls.load(Ordering::Acquire), 1);
        assert!(manager
            .coordinator
            .store
            .owner(&old_id)
            .await
            .unwrap()
            .is_some());
        assert!(manager
            .coordinator
            .acquire(&old_id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(healthy.run(async { 42 }).await.unwrap(), 42);
        receiver.acknowledges.store(true, Ordering::Release);
        // No caller supplies another hint or manually runs housekeeping. The
        // owned supervisor must revisit the retained noncurrent responsibility.
        tokio::time::timeout(Duration::from_secs(8), async {
            while manager
                .coordinator
                .owned()
                .await
                .iter()
                .any(|a| a.token == old.token)
            {
                assert!(manager.is_ready() && healthy.is_current());
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let next = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(next) = manager.coordinator.acquire(&old_id).await.unwrap() {
                    break next;
                }
                assert!(manager.is_ready() && healthy.is_current());
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        assert!(next.token.epoch > old.token.epoch && next.is_current());
        assert!(old.run(async { 42 }).await.is_err());
        assert_eq!(next.run(async { 42 }).await.unwrap(), 42);
        assert_eq!(receiver.calls.load(Ordering::Acquire), 2);
        assert!(manager.is_ready() && healthy.is_current());
        manager.drain().await.unwrap();
        assert!(manager.supervisor.lock().await.is_none());
        client.database(&name).drop().await.unwrap();
    }
}
