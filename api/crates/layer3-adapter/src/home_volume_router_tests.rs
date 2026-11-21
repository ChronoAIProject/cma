use super::*;
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

struct Store {
    scope: HomeVolumeScope,
    rows: Mutex<BTreeMap<String, HomeVolumeAttachment>>,
    calls: Mutex<Vec<(&'static str, HomeVolumeRequest)>>,
    lose_prepare: AtomicBool,
    fail_inventory: AtomicBool,
}
impl Store {
    fn new(cluster: &str) -> Arc<Self> {
        Arc::new(Self {
            scope: HomeVolumeScope::from_authority(
                "kubernetes-pvc-v1",
                &[cluster, "runtime", "deployment"],
            )
            .unwrap(),
            rows: Mutex::default(),
            calls: Mutex::default(),
            lose_prepare: AtomicBool::new(false),
            fail_inventory: AtomicBool::new(false),
        })
    }
    fn retained(&self, request: &HomeVolumeRequest, name: &str) -> HomeVolumeAttachment {
        let attachment = HomeVolumeAttachment {
            request: request.clone(),
            receipt: HomeVolumeReceipt {
                native_authority: None,
                volume_id: request.volume_id.clone(),
                request_sha256: request.computed_sha256(),
                storage_reference: name.into(),
                storage_generation: format!("uid-{name}"),
            },
        };
        self.rows
            .lock()
            .unwrap()
            .insert(request.volume_id.clone(), attachment.clone());
        attachment
    }
    fn call(&self, operation: &'static str, request: &HomeVolumeRequest) {
        self.calls
            .lock()
            .unwrap()
            .push((operation, request.clone()));
    }
}
#[async_trait::async_trait]
impl HomeVolumeStore for Store {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(self.scope.clone())
    }
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>> {
        if self.fail_inventory.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable("cluster unavailable".into()));
        }
        Ok(self.rows.lock().unwrap().values().cloned().collect())
    }
    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt> {
        self.call("prepare", request);
        let existing = self.rows.lock().unwrap().get(&request.volume_id).cloned();
        let home = existing.unwrap_or_else(|| self.retained(request, "original-pvc"));
        assert_eq!(&home.request, request);
        if self.lose_prepare.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "response lost after commit".into(),
            ));
        }
        Ok(home.receipt)
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> Result<()> {
        self.call("verify", &home.request);
        if self.rows.lock().unwrap().get(&home.request.volume_id) != Some(home) {
            return Err(rejected("wrong original home"));
        }
        Ok(())
    }
    async fn remove_captures(&self, home: &HomeVolumeAttachment) -> Result<()> {
        self.verify(home).await?;
        self.call("captures", &home.request);
        Ok(())
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<()> {
        self.call("remove", request);
        let mut rows = self.rows.lock().unwrap();
        if let Some(home) = rows.get(&request.volume_id) {
            assert_eq!(&home.request, request);
            if let Some(receipt) = receipt {
                assert_eq!(&home.receipt, receipt);
            }
        }
        rows.remove(&request.volume_id);
        Ok(())
    }
    async fn prepare_receipted(
        &self,
        request: &HomeVolumeRequest,
        operation_id: &str,
    ) -> Result<HomeProvisionReceipt> {
        Ok(HomeProvisionReceipt {
            provider_scope: self.scope.clone(),
            home: HomeVolumeAttachment {
                request: request.clone(),
                receipt: self.prepare(request).await?,
            },
            operation_id: operation_id.into(),
            creation_resource_version: "1".into(),
            creation_response_sha256: "a".repeat(64),
        })
    }
    async fn delete_original_home(
        &self,
        original: &HomeProvisionReceipt,
        operation_id: &str,
    ) -> Result<HomeDeleteReceipt> {
        self.remove(&original.home.request, Some(&original.home.receipt))
            .await?;
        Ok(HomeDeleteReceipt {
            original: original.clone(),
            operation_id: operation_id.into(),
            resource_version: "2".into(),
            response_sha256: "b".repeat(64),
        })
    }
    async fn observe_home_finality(
        &self,
        delete: &HomeDeleteReceipt,
    ) -> Result<HomeFinalityReceipt> {
        self.call("finality", &delete.original.home.request);
        assert!(!self
            .rows
            .lock()
            .unwrap()
            .contains_key(&delete.original.home.request.volume_id));
        Ok(HomeFinalityReceipt {
            delete: delete.clone(),
            claim_absence_observation_sha256: HomeFinalityReceipt::absence_digest(delete)?,
            cleanup_scope: crate::HomeCleanupScope::NamespacedClaim,
        })
    }
}
fn policy(name: &str) -> HomeVolumePolicy {
    HomeVolumePolicy {
        mode: Default::default(),
        storage_class: format!("{name}-storage"),
        capacity_gib: 16,
        template_name: format!("{name}-seed"),
        template_generation: format!("{name}-seed-uid"),
    }
}
fn request(id: &str) -> HomeVolumeRequest {
    HomeVolumeRequest {
        volume_id: id.into(),
        sandbox_id: format!("sandbox-{id}"),
        policy: policy("historical"),
        scope: None,
    }
}
fn router(local: &Arc<Store>, remote: &Arc<Store>) -> RoutedHomeVolumes {
    RoutedHomeVolumes::new(
        Some((local.clone(), policy("local"))),
        Some((remote.clone(), policy("remote"))),
    )
    .unwrap()
}

#[test]
fn configured_authorities_are_recognized_without_selecting_an_allocation_default() {
    let local = Store::new("local");
    let remote = Store::new("remote");
    let foreign = Store::new("foreign");
    let router = router(&local, &remote);
    assert!(router.allocation_scope().is_none());
    assert!(router.accepts_scope(&local.scope));
    assert!(router.accepts_scope(&remote.scope));
    assert!(!router.accepts_scope(&foreign.scope));
    let remote_only =
        RoutedHomeVolumes::new(None, Some((remote.clone(), policy("remote")))).unwrap();
    assert!(remote_only.accepts_scope(&remote.scope));
    assert!(!remote_only.accepts_scope(&local.scope));
    assert!(local.calls.lock().unwrap().is_empty() && remote.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cold_native_intent_survives_lost_response_reopen_and_seed_configuration_change() {
    let local = Store::new("local");
    let remote = Store::new("remote");
    let router = router(&local, &remote);
    let reserved = router.reserve_for(true, &request("native")).unwrap();
    assert_eq!(reserved.policy, policy("remote"));
    assert_eq!(reserved.scope.as_ref(), Some(&remote.scope));
    assert!(local.calls.lock().unwrap().is_empty() && remote.calls.lock().unwrap().is_empty());
    let persisted = serde_json::to_vec(&reserved).unwrap();
    remote.lose_prepare.store(true, Ordering::SeqCst);
    assert!(router.prepare(&reserved).await.is_err());
    let reopened = RoutedHomeVolumes::new(
        Some((local.clone(), policy("local-new"))),
        Some((remote.clone(), policy("remote-new"))),
    )
    .unwrap();
    let retained: HomeVolumeRequest = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(reopened.reserve_for(true, &retained).unwrap(), reserved);
    assert!(reopened.reserve_for(false, &retained).is_err());
    let home = HomeVolumeAttachment {
        receipt: reopened.prepare(&retained).await.unwrap(),
        request: retained,
    };
    reopened.verify(&home).await.unwrap();
    reopened.remove_captures(&home).await.unwrap();
    reopened
        .remove(&home.request, Some(&home.receipt))
        .await
        .unwrap();
    assert!(local.calls.lock().unwrap().is_empty());
    assert!(remote.rows.lock().unwrap().is_empty());
    assert!(remote
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|(_, request)| request == &reserved));
}

#[tokio::test]
async fn warm_adopted_pvc_and_legacy_unattached_home_keep_original_cluster_on_release() {
    let local = Store::new("local");
    let remote = Store::new("remote");
    let router = router(&local, &remote);
    let legacy = request("legacy");
    let old_home = local.retained(&legacy, "historical-pvc");
    let warm = router.reserve_for(true, &request("warm")).unwrap();
    let adopted = remote.retained(&warm, "home-original-warm-member");
    let reopened: HomeVolumeAttachment =
        serde_json::from_slice(&serde_json::to_vec(&adopted).unwrap()).unwrap();
    router.verify(&reopened).await.unwrap();
    router.verify(&old_home).await.unwrap();
    assert_eq!(router.inventory().await.unwrap().len(), 2);
    router.remove(&legacy, None).await.unwrap(); // response was lost before a legacy receipt was saved
    router
        .remove(&reopened.request, Some(&reopened.receipt))
        .await
        .unwrap();
    assert_eq!(
        local.calls.lock().unwrap().last().unwrap(),
        &("remove", legacy)
    );
    assert_eq!(
        remote.calls.lock().unwrap().last().unwrap(),
        &("remove", warm)
    );
    assert!(local.rows.lock().unwrap().is_empty() && remote.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_ambiguous_missing_and_conflicting_home_authority_never_falls_back() {
    let local = Store::new("local");
    let remote = Store::new("remote");
    assert!(RoutedHomeVolumes::new(
        Some((local.clone(), policy("a"))),
        Some((local.clone(), policy("b")))
    )
    .is_err());
    let router = router(&local, &remote);
    let mut unknown = request("foreign");
    unknown.scope = Some(Store::new("unknown").scope.clone());
    assert!(router.prepare(&unknown).await.is_err());
    assert!(router.remove(&unknown, None).await.is_err());
    let remote_only =
        RoutedHomeVolumes::new(None, Some((remote.clone(), policy("remote")))).unwrap();
    assert!(remote_only.prepare(&request("old")).await.is_err());
    let reserved = router.reserve_for(true, &request("pinned")).unwrap();
    let replacement = RoutedHomeVolumes::new(
        Some((local.clone(), policy("local"))),
        Some((Store::new("replacement-cluster"), policy("remote"))),
    )
    .unwrap();
    assert!(replacement.prepare(&reserved).await.is_err());
    assert!(local.calls.lock().unwrap().is_empty() && remote.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn combined_inventory_rejects_duplicate_allocations_foreign_scopes_and_incomplete_reads() {
    let local = Store::new("local");
    let remote = Store::new("remote");
    let router = router(&local, &remote);
    let legacy = request("duplicate");
    local.retained(&legacy, "local-pvc");
    remote.retained(&router.reserve_for(true, &legacy).unwrap(), "remote-pvc");
    assert!(router.inventory().await.is_err());
    remote.rows.lock().unwrap().clear();
    remote.retained(&request("unscoped-remote"), "remote-pvc");
    assert!(router.inventory().await.is_err());
    remote.rows.lock().unwrap().clear();
    remote.fail_inventory.store(true, Ordering::SeqCst);
    assert!(router.inventory().await.is_err());
    assert!(local.calls.lock().unwrap().is_empty() && remote.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn original_creation_deletion_and_finality_receipts_keep_their_provider_scope() {
    let local = Store::new("local");
    let remote = Store::new("remote");
    let router = router(&local, &remote);
    let reserved = router.reserve_for(true, &request("original")).unwrap();
    let original = router
        .prepare_receipted(&reserved, "create-original")
        .await
        .unwrap();
    let mut changed = original.clone();
    changed.provider_scope = local.scope.clone();
    assert!(router
        .delete_original_home(&changed, "delete")
        .await
        .is_err());
    let deleted = router
        .delete_original_home(&original, "delete")
        .await
        .unwrap();
    let resumed: HomeDeleteReceipt =
        serde_json::from_slice(&serde_json::to_vec(&deleted).unwrap()).unwrap();
    let finality = router.observe_home_finality(&resumed).await.unwrap();
    assert_eq!(finality.original(), &original);
    assert!(local.calls.lock().unwrap().is_empty());
}

#[test]
fn legacy_request_json_and_digest_remain_unchanged_while_scope_changes_identity() {
    let value = request("legacy");
    let encoded = serde_json::to_value(&value).unwrap();
    assert!(encoded.get("scope").is_none());
    assert_eq!(
        serde_json::from_value::<HomeVolumeRequest>(encoded).unwrap(),
        value
    );
    assert_eq!(
        value.computed_sha256(),
        "db13a5b1909f08d588c7134c29660569113d35b375c03a4b49b973b333ffa8bd"
    );
    let home = Store::new("local").retained(&value, "historical-pvc");
    assert_eq!(
        home.identity().unwrap(),
        "KQ4Y1DTKODxqJ4XgAha7aSd5fB3NhIjRsNw8kpGYk4s"
    );
    let mut scoped = value.clone();
    scoped.scope = Some(Store::new("remote").scope.clone());
    assert_ne!(scoped.computed_sha256(), value.computed_sha256());
    scoped.scope = Some(Store::new("local").scope.clone());
    assert_ne!(scoped.computed_sha256(), value.computed_sha256());
}
