use super::*;
use layer2_core::agent_ownership::Authority;
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

struct Receiver {
    entered: Notify,
    resume: Notify,
    calls: AtomicUsize,
    lifecycle_exited: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "contained-drain-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        assert!(self.lifecycle_exited.load(Ordering::Acquire));
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(())
    }
}

struct Exited(Arc<AtomicBool>);
impl Drop for Exited {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn fixture(
    pacing: bool,
    receiver_only: bool,
) -> (
    mongodb::Client,
    String,
    Arc<AgentManager>,
    Arc<Authority>,
    Arc<Receiver>,
) {
    let (client, name, store) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let store = Arc::new(
        store
            .enable_agent_ownership("contained-drain-member".into())
            .await
            .unwrap(),
    );
    let manager = AgentManager::register_with_policy(
        store,
        Arc::new(LiveAgentRegistry::default()),
        "http://manager.invalid:8080".into(),
        2,
        pacing,
        MaintenancePolicy::ContainFailures,
    )
    .await
    .unwrap();
    if receiver_only {
        manager.enable_receiver_containment();
    }
    let lifecycle = crate::agent_lifecycle::LifecycleDrain::fixture(1);
    let exited = Arc::new(AtomicBool::new(false));
    let guard = Exited(exited.clone());
    assert!(lifecycle.spawn_fixture(async move {
        let _guard = guard;
        std::future::pending::<()>().await;
    }));
    manager.register_lifecycle(lifecycle).unwrap();
    assert!(!manager.coordinator.is_registered());
    assert!(!manager.is_ready());
    assert!(manager.supervisor.lock().await.is_none());
    assert!(manager
        .coordinator
        .store
        .membership()
        .await
        .unwrap()
        .members
        .is_empty());
    let (first, concurrent) = tokio::join!(manager.activate(), manager.activate());
    first.unwrap();
    concurrent.unwrap();
    let task = manager.supervisor.lock().await.as_ref().unwrap().id();
    manager.activate().await.unwrap();
    assert_eq!(manager.supervisor.lock().await.as_ref().unwrap().id(), task);
    let authority = manager
        .coordinator
        .acquire(&AgentId("contained-drain-agent".into()))
        .await
        .unwrap()
        .unwrap();
    let receiver = Arc::new(Receiver {
        entered: Notify::new(),
        resume: Notify::new(),
        calls: AtomicUsize::new(0),
        lifecycle_exited: exited,
    });
    authority.register_receiver(receiver.clone()).unwrap();
    authority
        .begin_receiver_confirmation(&receiver.identity())
        .unwrap();
    authority.receiver_confirmed(&receiver.identity());
    (client, name, manager, authority, receiver)
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn cancelled_prestop_and_process_shutdown_share_actual_manager_episode_and_complete_custody()
{
    for (pacing, receiver_only) in [false, true]
        .into_iter()
        .flat_map(|pacing| [false, true].map(|receiver_only| (pacing, receiver_only)))
    {
        let (client, name, manager, authority, receiver) = fixture(pacing, receiver_only).await;
        let first = tokio::spawn({
            let manager = manager.clone();
            async move { manager.drain().await }
        });
        tokio::time::timeout(Duration::from_secs(3), receiver.entered.notified())
            .await
            .unwrap();
        let original_until = manager.drain_episode.get().unwrap().until;
        assert!(!manager.is_ready() && !authority.is_current());
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert!(manager
            .coordinator
            .store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .is_some());
        receiver.resume.notify_one();
        manager
            .drain_contained(
                DrainCause::Shutdown,
                Instant::now() + Duration::from_secs(63),
            )
            .await
            .unwrap();
        assert_eq!(manager.drain_episode.get().unwrap().until, original_until);
        assert_eq!(receiver.calls.load(Ordering::Acquire), 2);
        assert!(manager.supervisor.lock().await.is_none());
        assert!(manager.coordinator.owned().await.is_empty());
        assert!(manager
            .coordinator
            .store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .is_none());
        manager.drain().await.unwrap();
        assert_eq!(receiver.calls.load(Ordering::Acquire), 2);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn supervisor_fatal_result_is_retained_after_successful_cleanup() {
    let (client, name, manager, authority, receiver) = fixture(false, true).await;
    // The actual supervisor's result is observed independently of the failure
    // watch. Simulate a fatal return racing the start of shutdown.
    let supervisor = manager.supervisor.lock().await.take().unwrap();
    supervisor.abort();
    assert!(supervisor.await.unwrap_err().is_cancelled());
    *manager.supervisor.lock().await = Some(tokio::spawn(async { Err(()) }));
    let first = tokio::spawn({
        let manager = manager.clone();
        async move { manager.drain().await }
    });
    tokio::time::timeout(Duration::from_secs(3), receiver.entered.notified())
        .await
        .unwrap();
    assert!(manager.supervisor.lock().await.is_none());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    receiver.resume.notify_one();
    let error = manager.drain().await.unwrap_err();
    assert_eq!(
        error,
        "Agent Manager supervisor reported an ownership failure"
    );
    assert!(manager.coordinator.owned().await.is_empty());
    assert!(manager
        .coordinator
        .store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(manager.supervisor.lock().await.is_none());
    assert_eq!(manager.drain().await.unwrap_err(), error);
    client.database(&name).drop().await.unwrap();
}
