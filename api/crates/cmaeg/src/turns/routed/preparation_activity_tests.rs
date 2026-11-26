//! A target that cannot be prepared stays queued and names why on Activity.
use super::{test_support::*, *};
use crate::{
    activity::{ActivityStore, Change, ConsumerState},
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{
        test_server::{Row, Server},
        AdmittedPlan, StoredTarget,
    },
};
use cmaeg_core::{
    channel::{ProviderBinding, Secret},
    seal::seal_blob,
};
use futures_util::future::BoxFuture;
use std::sync::{atomic::Ordering, Arc, Mutex};

/// Conversation preparation fails with each queued error in turn.
struct FailingPreparation {
    base: Arc<Port>,
    failures: Mutex<Vec<ExecutionError>>,
}
impl ExecutionPort for FailingPreparation {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport {
        self.base.support(binding)
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            if let Some(error) = self.failures.lock().unwrap().pop() {
                return Err(error);
            }
            ExecutionPort::prepare(self.base.as_ref(), plan, target, current).await
        })
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        self.base.start(body, binding, credential)
    }
}

#[tokio::test]
async fn unprepared_targets_stay_queued_with_a_named_reason() {
    let record = uuid::Uuid::new_v4().to_string();
    let request = || {
        fixture::request(
            |value| {
                fixture::targets(value);
                value["record_id"] = record.clone().into();
            },
            fixture::message("hello"),
        )
    };
    let original = request();
    let owner = original.channel.definition.owner.subject.clone();
    let mut setup = Setup::with_request(supported(), request()).await;
    let consumer_id = setup.target().launch.consumer.to_string();
    let activity_records = Server::start_realtime().await;
    activity_records.memory.channels.lock().unwrap().insert(
        record.clone(),
        Row {
            owner: owner.clone(),
            blob: seal_blob(
                &fixture::keys(),
                &serde_json::to_vec(&original.channel.definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
            expiry: time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
            version: 1,
        },
    );
    let activity = Arc::new(ActivityStore::new(
        activity_records.records.clone(),
        Arc::new(fixture::keys()),
    ));
    activity.received(&original).await.unwrap();
    activity
        .change(
            &record,
            original.activity.event_id.as_str(),
            &Change::state(&consumer_id, ConsumerState::Queued, None),
        )
        .await
        .unwrap();
    // Popped from the end: busy first, then unavailable, then a lost lease.
    let port = Arc::new(FailingPreparation {
        base: setup.port.clone(),
        failures: Mutex::new(vec![
            ExecutionError::LostLease,
            ExecutionError::Unavailable,
            ExecutionError::Busy,
        ]),
    });
    setup.dispatcher = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone().with_activity(activity.clone()),
        setup.current.clone(),
        port,
    );
    let consumer = || async {
        activity.latest(&owner, &record).await.unwrap().items[0]
            .consumers
            .iter()
            .find(|consumer| consumer.consumer_id == consumer_id)
            .unwrap()
            .clone()
    };
    for (expected, code) in [
        (ExecutionError::Busy, "preparation_provider_busy"),
        (
            ExecutionError::Unavailable,
            "preparation_provider_unavailable",
        ),
    ] {
        assert!(matches!(setup.dispatch().await, Err(error) if error == expected));
        let consumer = consumer().await;
        assert_eq!(consumer.state, ConsumerState::Queued);
        assert_eq!(consumer.last_non_admission_code.as_deref(), Some(code));
        assert!(consumer.dispatched_at.is_none());
        // Let the lease lapse so the next attempt can claim it.
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    }
    // A lost lease belongs to the worker that took over: no new reason.
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::LostLease)
    ));
    assert_eq!(
        consumer().await.last_non_admission_code.as_deref(),
        Some("preparation_provider_unavailable")
    );
    assert!(setup.port.starts.lock().unwrap().is_empty());
}

#[test]
fn every_preparation_failure_but_a_lost_lease_has_a_closed_code() {
    use super::dispatcher::preparation_failure_code;
    assert_eq!(preparation_failure_code(&ExecutionError::LostLease), None);
    for error in [
        ExecutionError::Abandoned,
        ExecutionError::IdempotencyConflict,
        ExecutionError::OperationLimit,
        ExecutionError::Invalid,
        ExecutionError::Unavailable,
        ExecutionError::Uncertain,
        ExecutionError::Corrupt,
        ExecutionError::Busy,
        ExecutionError::Expired,
        ExecutionError::AuthorityChanged,
        ExecutionError::Unsupported,
    ] {
        let code = preparation_failure_code(&error).unwrap();
        assert!(
            code.starts_with("preparation_") && code.len() <= 64,
            "{code}"
        );
    }
}
