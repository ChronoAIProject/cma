use super::*;
use layer2_core::sandbox::SandboxAutoPauseService;
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Caller,
    Capability, DetailKey, PlatformSettingsService, StoreError,
};
use std::time::Duration;

struct CrashBeforeOutcome {
    durable: Arc<crate::store::activity::MongoActivityStore>,
    reached: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl ActivityStore for CrashBeforeOutcome {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        if record.detail.get(DetailKey::Status) != Some("requested") {
            self.reached.notify_one();
            std::future::pending::<()>().await;
        }
        self.durable.append(record).await
    }
    async fn list(
        &self,
        query: &ActivityQuery,
    ) -> Result<layer2_core::Page<ActivityRecord>, StoreError> {
        self.durable.list(query).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_crash_after_pause_keeps_durable_actor_intent_and_restart_reconciles_effect() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_audit_crash").await;
    let (settings, _) = policy_tests::enable(&client, &name).await;
    let durable = Arc::new(
        crate::store::activity::MongoActivityStore::open(
            &client,
            &name,
            Duration::from_secs(86400),
        )
        .await
        .unwrap(),
    );
    let crash = Arc::new(CrashBeforeOutcome {
        durable,
        reached: tokio::sync::Notify::new(),
    });
    let activity = ActivityService::new(crash.clone());
    let service = Arc::new(SandboxAutoPauseService::new(
        Arc::new(PlatformSettingsService::new(
            Arc::new(settings),
            activity.clone(),
        )),
        Arc::new(store.clone()),
        activity,
    ));
    let caller = Caller {
        subject: SubjectId::new("operator-crash"),
        authorization: Authorization::new([Capability::ConfigurePlatform]),
    };
    let actor = caller.clone();
    let task = tokio::spawn(async move {
        service
            .batch(&actor, PageRequest::new(1, None).unwrap(), true)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), crash.reached.notified())
        .await
        .unwrap();
    // Simulate process loss after the real Mongo pause commits and before the
    // outcome write. Intent is already majority durable on another store.
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
    let activity = crate::store::activity::MongoActivityStore::open(
        &client,
        &name,
        Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let records = activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].actor.as_ref(), Some(&caller.subject));
    assert_eq!(records[0].detail.get(DetailKey::Status), Some("requested"));
    assert_eq!(
        records[0].target,
        Some(layer2_core::ActivityTarget::Resource(format!(
            "sandbox:{}",
            sandbox.id
        )))
    );
    let events = store.sandbox_events(&sandbox.id, 0, 100).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].appended_at, records[0].at);
    let admitted = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(
        admitted
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .requested_at,
        records[0].at
    );
    // A repeated bounded batch must not duplicate the admitted pause. Pausing
    // is no longer a candidate, so recovery is observed through its durable
    // event rather than reporting a fresh scheduled effect.
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let activity = ActivityService::new(Arc::new(activity));
    let settings = crate::store::settings::MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    let restarted = SandboxAutoPauseService::new(
        Arc::new(PlatformSettingsService::new(
            Arc::new(settings),
            activity.clone(),
        )),
        Arc::new(replica.clone()),
        activity,
    );
    let batch = restarted
        .batch(&caller, PageRequest::new(1, None).unwrap(), true)
        .await
        .unwrap();
    assert_eq!((batch.scheduled, batch.failed), (0, 0));
    assert_eq!(
        replica.sandbox_events(&sandbox.id, 0, 100).await.unwrap(),
        events
    );
    assert_eq!(
        replica.get_sandbox(&sandbox.id).await.unwrap(),
        store.get_sandbox(&sandbox.id).await.unwrap()
    );
    client.database(&name).drop().await.unwrap();
}
