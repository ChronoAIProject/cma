use super::*;
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope,
    HomeVolumeStore, InventoryCursor, RoutedHomeVolumes,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Homes {
    scope: HomeVolumeScope,
    rows: Vec<HomeVolumeAttachment>,
    calls: Arc<Mutex<Vec<usize>>>,
    removed: Arc<Mutex<Vec<String>>>,
    failure: Arc<Mutex<Option<(usize, Failure)>>>,
}

impl Homes {
    fn new(route: &str, count: usize) -> Self {
        let scope = HomeVolumeScope::from_authority("kubernetes-pvc-v1", &[route]).unwrap();
        let rows = (0..count)
            .map(|i| {
                let request = HomeVolumeRequest {
                    scope: Some(scope.clone()),
                    volume_id: format!("home-{route}-{i}"),
                    sandbox_id: format!("sbx_{route}_{i}"),
                    policy: policy(),
                };
                let receipt = HomeVolumeReceipt {
                    native_authority: None,
                    volume_id: request.volume_id.clone(),
                    request_sha256: request.computed_sha256(),
                    storage_reference: format!("pvc-{route}-{i}"),
                    storage_generation: format!("uid-{route}-{i}"),
                };
                HomeVolumeAttachment { request, receipt }
            })
            .collect();
        Self {
            scope,
            rows,
            calls: Default::default(),
            removed: Default::default(),
            failure: Default::default(),
        }
    }
}

fn policy() -> HomeVolumePolicy {
    HomeVolumePolicy {
        mode: Default::default(),
        storage_class: "disk".into(),
        capacity_gib: 1,
        template_name: "seed".into(),
        template_generation: "seed-uid".into(),
    }
}

struct Cursor {
    homes: Homes,
    index: usize,
    items: Vec<HomeVolumeAttachment>,
}

#[async_trait::async_trait]
impl InventoryCursor<HomeVolumeAttachment> for Cursor {
    async fn advance(&mut self) -> layer3_adapter::Result<Option<Vec<HomeVolumeAttachment>>> {
        if self.index == self.homes.rows.len() {
            return Ok(Some(std::mem::take(&mut self.items)));
        }
        self.homes.calls.lock().unwrap().push(self.index);
        let failure = {
            let mut failure = self.homes.failure.lock().unwrap();
            if failure.is_some_and(|(index, _)| index == self.index) {
                failure.take()
            } else {
                None
            }
        };
        match failure.map(|(_, failure)| failure) {
            Some(Failure::Deadline) => std::future::pending::<()>().await,
            Some(_) => {
                return Err(layer3_adapter::AdapterError::Rejected(
                    "invalid inventory fixture".into(),
                ))
            }
            None => tokio::time::sleep(Duration::from_secs(2)).await,
        }
        self.items.push(self.homes.rows[self.index].clone());
        self.index += 1;
        Ok(None)
    }
}

#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(self.scope.clone())
    }
    fn inventory_cursor(&self) -> Option<Box<dyn InventoryCursor<HomeVolumeAttachment>>> {
        Some(Box::new(Cursor {
            homes: self.clone(),
            index: 0,
            items: vec![],
        }))
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        panic!("bounded maintenance must use the incremental provider path")
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        unreachable!()
    }
    async fn verify(&self, _: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        unreachable!()
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        self.removed.lock().unwrap().push(request.volume_id.clone());
        Ok(())
    }
}

async fn reconciler(local: &Homes, native: &Homes) -> (Arc<fixtures::Store>, SandboxReconciler) {
    let (store, mut reconciler) = setup(0, Duration::ZERO).await;
    let routes = RoutedHomeVolumes::new(
        Some((Arc::new(local.clone()), policy())),
        Some((Arc::new(native.clone()), policy())),
    )
    .unwrap();
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    reconciler.adapter = Arc::new(adapter.with_home_volumes(Arc::new(routes)));
    (store, reconciler)
}

#[tokio::test(start_paused = true)]
async fn home_inventory_spans_attempts_and_exposes_only_a_complete_set() {
    let local = Homes::new("local", 6);
    let native = Homes::new("native", 6);
    let (_, reconciler) = reconciler(&local, &native).await;
    let started = Instant::now();
    assert!(!reconciler.run_due(SystemTime::now()).await.unwrap());
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .phase,
        Phase::HomeInventory
    );
    assert!(local.removed.lock().unwrap().is_empty());
    assert!(native.removed.lock().unwrap().is_empty());
    complete(&reconciler).await;
    assert!(started.elapsed() >= Duration::from_secs(24));
    for homes in [&local, &native] {
        assert_eq!(*homes.calls.lock().unwrap(), (0..6).collect::<Vec<_>>());
        assert_eq!(
            *homes.removed.lock().unwrap(),
            homes
                .rows
                .iter()
                .map(|h| h.request.volume_id.clone())
                .collect::<Vec<_>>()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn deadline_or_invalid_item_retains_route_and_item_without_partial_cleanup() {
    for failure in [Failure::Deadline, Failure::Corrupt] {
        let local = Homes::new("local", 2);
        let native = Homes::new("native", 4);
        *native.failure.lock().unwrap() = Some((2, failure));
        let (_, reconciler) = reconciler(&local, &native).await;
        assert!(reconciler.run_due(SystemTime::now()).await.is_err());
        assert!(local.removed.lock().unwrap().is_empty());
        assert!(native.removed.lock().unwrap().is_empty());
        complete(&reconciler).await;
        assert_eq!(*local.calls.lock().unwrap(), [0, 1]);
        assert_eq!(*native.calls.lock().unwrap(), [0, 1, 2, 2, 3]);
        assert_eq!(local.removed.lock().unwrap().len(), 2);
        assert_eq!(native.removed.lock().unwrap().len(), 4);
    }
}

#[tokio::test(start_paused = true)]
async fn resumed_home_cleanup_rechecks_newly_published_ownership() {
    let local = Homes::new("local", 1);
    let native = Homes::new("native", 0);
    let (store, reconciler) = reconciler(&local, &native).await;
    let home = &local.rows[0];
    store.fail("get", &home.request.sandbox_id, Failure::Deadline);
    assert!(reconciler.run_due(SystemTime::now()).await.is_err());
    assert!(local.removed.lock().unwrap().is_empty());
    let mut owned = sandbox(0);
    owned.id = SandboxId(home.request.sandbox_id.clone());
    owned.lifecycle.home = Some(Box::new(layer2_core::sandbox::SandboxHome {
        request: home.request.clone(),
        receipt: Some(home.receipt.clone()),
        released: false,
        native_pool: None,
    }));
    owned.validate().unwrap();
    store.inner.create_sandbox(owned).await.unwrap();
    complete(&reconciler).await;
    assert!(local.removed.lock().unwrap().is_empty());
    assert_eq!(*local.calls.lock().unwrap(), [0]);
    assert_eq!(
        store.keys("get"),
        [
            home.request.sandbox_id.clone(),
            home.request.sandbox_id.clone()
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn held_binding_keeps_its_home_while_independent_orphan_cleanup_continues() {
    let local = Homes::new("local", 2);
    let native = Homes::new("native", 0);
    let (store, reconciler) = reconciler(&local, &native).await;
    let mut held = sandbox(0);
    held.id = SandboxId(local.rows[0].request.sandbox_id.clone());
    store.inner.create_sandbox(held.clone()).await.unwrap();
    store.fail("get", held.id.as_str(), Failure::Unavailable);
    complete(&reconciler).await;
    assert_eq!(
        *local.removed.lock().unwrap(),
        [local.rows[1].request.volume_id.clone()]
    );
    assert_eq!(
        store
            .keys("get")
            .iter()
            .filter(|id| *id == held.id.as_str())
            .count(),
        1
    );
    assert!(reconciler.binding_holds.contains(held.id.as_str()));
}
