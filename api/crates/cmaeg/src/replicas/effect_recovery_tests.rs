use super::*;
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::{effects::EffectDisposition, partition::PartitionLease},
    turns::routed::ExecutionError,
};
use std::{collections::VecDeque, sync::Mutex as SyncMutex};

#[derive(Default)]
struct Frontier(SyncMutex<Vec<Option<String>>>);
impl RecoverySource for Frontier {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, ScanError>> {
        self.0.lock().unwrap().push(after.map(str::to_owned));
        Box::pin(async {
            Ok(RecoveryPage {
                plans: Vec::new(),
                issues: Vec::new(),
                next_cursor: Some("next-partition".into()),
            })
        })
    }
}

struct Effects {
    seen: SyncMutex<Vec<Option<String>>>,
    steps: SyncMutex<VecDeque<Result<Option<String>, ExecutionError>>>,
}
impl Effects {
    fn new(steps: impl IntoIterator<Item = Result<Option<String>, ExecutionError>>) -> Self {
        Self {
            seen: SyncMutex::new(Vec::new()),
            steps: SyncMutex::new(steps.into_iter().collect()),
        }
    }
}
impl EffectExecutor for Effects {
    fn execute<'a>(
        &'a self,
        _: &'a AdmittedPlan,
        _: &'a StoredTarget,
        _: &'a str,
        _: &'a mut PartitionLease,
    ) -> BoxFuture<'a, Result<EffectDisposition, ExecutionError>> {
        panic!("discovery cannot execute a provider effect")
    }
    fn recover<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>, ExecutionError>> {
        self.seen.lock().unwrap().push(after.map(str::to_owned));
        let step = self.steps.lock().unwrap().pop_front();
        Box::pin(async move {
            match step {
                Some(result) => result,
                None => std::future::pending().await,
            }
        })
    }
}

#[tokio::test]
async fn separate_effect_cursor_retries_failure_without_stalling_queued_frontier() {
    let frontier = Arc::new(Frontier::default());
    let effects = Arc::new(Effects::new([
        Ok(Some("next-effect".into())),
        Err(ExecutionError::Unavailable),
        Ok(None),
    ]));
    let recovery = Recovery::new(frontier.clone(), effects.clone());
    for _ in 0..3 {
        let page = recovery.page(Some("partition-cursor")).await.unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("next-partition"));
    }
    assert_eq!(
        *effects.seen.lock().unwrap(),
        [None, Some("next-effect".into()), Some("next-effect".into())]
    );
    assert_eq!(
        *frontier.0.lock().unwrap(),
        vec![Some("partition-cursor".into()); 3]
    );
    assert!(recovery.cursor.lock().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn stuck_effect_repair_is_bounded_and_retains_cursor_without_blocking_frontier() {
    let frontier = Arc::new(Frontier::default());
    let effects = Arc::new(Effects::new([Ok(Some("pending-effect".into()))]));
    let recovery = Recovery::new(frontier.clone(), effects.clone());
    recovery.page(None).await.unwrap();
    let started = tokio::time::Instant::now();
    recovery.page(Some("next-partition")).await.unwrap();
    assert_eq!(started.elapsed(), STORE_TIMEOUT);
    assert_eq!(
        recovery.cursor.lock().await.as_deref(),
        Some("pending-effect")
    );
    assert_eq!(frontier.0.lock().unwrap().len(), 2);
    assert_eq!(effects.seen.lock().unwrap().len(), 2);
}
