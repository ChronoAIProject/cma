use super::*;
use crate::warm_pools::tests::target;
use async_trait::async_trait;
use layer3_adapter::agent_sandbox::NativePoolBinding;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

struct Store {
    targets: Mutex<Vec<PoolTarget>>,
    conflict: AtomicBool,
    resized: Mutex<Vec<(String, u64, u32)>>,
}

#[async_trait]
impl PoolTargetStore for Store {
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError> {
        Ok(self.targets.lock().unwrap().clone())
    }
    async fn find(&self, _: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        unreachable!()
    }
    async fn insert(&self, _: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        unreachable!()
    }
    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        unreachable!()
    }
    async fn bind(&self, _: &PoolTarget, _: &NativePoolBinding) -> Result<(), WarmPoolError> {
        unreachable!()
    }
    async fn finish(&self, _: &PoolTarget, _: bool) -> Result<(), WarmPoolError> {
        unreachable!()
    }
    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
    ) -> Result<PoolTarget, WarmPoolError> {
        self.resized.lock().unwrap().push((
            id.into(),
            body.expected_revision,
            body.target_replicas,
        ));
        if self.conflict.swap(false, Ordering::SeqCst) {
            return Err(WarmPoolError::Conflict);
        }
        let mut targets = self.targets.lock().unwrap();
        let target = targets.iter_mut().find(|target| target.id == id).unwrap();
        assert_eq!(target.revision as u64, body.expected_revision);
        target.target_replicas = body.target_replicas;
        target.revision += 1;
        Ok(target.clone())
    }
}

#[tokio::test]
async fn obsolete_capacity_is_fenced_and_retried_without_touching_current_or_historical_authority()
{
    let old = target();
    let mut current = old.clone();
    current.id = "current".into();
    let mut disabled = old.clone();
    disabled.id = "disabled".into();
    disabled.target_replicas = 0;
    let store = Store {
        targets: Mutex::new(vec![old.clone(), current.clone(), disabled.clone()]),
        conflict: AtomicBool::new(true),
        resized: Mutex::new(vec![]),
    };
    let keep = |target: &PoolTarget| Ok(target.id == "current");
    assert!(!retire_superseded(&store, keep).await.unwrap());
    assert_eq!(
        store.targets.lock().unwrap()[0].target_replicas,
        old.target_replicas
    );
    assert!(retire_superseded(&store, keep).await.unwrap());
    assert!(retire_superseded(&store, keep).await.unwrap());
    assert_eq!(
        *store.resized.lock().unwrap(),
        vec![(old.id.clone(), 1, 0), (old.id.clone(), 1, 0)]
    );
    let records = store.targets.lock().unwrap();
    assert_eq!(records[0].target_replicas, 0);
    assert_eq!(records[0].revision, 2);
    assert_eq!(records[0].plan, old.plan);
    assert_eq!(records[0].binding, old.binding);
    assert_eq!(records[0].requested, old.requested);
    assert_eq!(records[1].target_replicas, current.target_replicas);
    assert_eq!(records[1].revision, current.revision);
    assert_eq!(records[2].revision, disabled.revision);
}

#[tokio::test]
async fn unknown_policy_does_not_disable_capacity() {
    let store = Store {
        targets: Mutex::new(vec![target()]),
        conflict: AtomicBool::new(false),
        resized: Mutex::new(vec![]),
    };
    assert_eq!(
        retire_superseded(&store, |_| Err(WarmPoolError::Unavailable)).await,
        Err(WarmPoolError::Unavailable)
    );
    assert!(store.resized.lock().unwrap().is_empty());
}
