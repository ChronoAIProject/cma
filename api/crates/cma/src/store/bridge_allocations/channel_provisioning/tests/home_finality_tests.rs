use super::*;
use crate::sandbox::channel_provisioning::{ChannelProvisioningDependencies, ChannelProvisioningDriver};
use crate::store::bridge_definitions::mongo_integration_tests::allocations::channel_provisioning::{fixture::VerifiedHuman, home};
use layer3_adapter::{codex::{CodexAdapter, CodexSandboxSpec}, *};
use serde_json::json;
use std::{collections::BTreeMap, sync::{Arc, atomic::{AtomicBool, AtomicUsize}}};

struct Homes {
    database: mongodb::Database,
    stored: tokio::sync::Mutex<BTreeMap<String, HomeVolumeAttachment>>,
    pending: AtomicBool,
    lose_create: AtomicBool,
    deletes: AtomicUsize,
    lose_delete: AtomicBool,
    captures: AtomicUsize,
}
#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(HomeVolumeScope::from_authority("home-finality-fixture", &["original"]).unwrap())
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.stored.lock().await.values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        let mut stored = self.stored.lock().await;
        Ok(stored
            .entry(request.volume_id.clone())
            .or_insert_with(|| HomeVolumeAttachment {
                request: request.clone(),
                receipt: HomeVolumeReceipt {
                    volume_id: request.volume_id.clone(),
                    request_sha256: request.computed_sha256(),
                    storage_reference: "original-claim".into(),
                    storage_generation: "original-claim-uid".into(),
                },
            })
            .receipt
            .clone())
    }
    async fn prepare_receipted(
        &self,
        request: &HomeVolumeRequest,
        operation_id: &str,
    ) -> layer3_adapter::Result<HomeProvisionReceipt> {
        let row = self
            .database
            .collection::<Document>("channel_provider_dispatch_journals")
            .find_one(doc! {"record.home.operation_id":operation_id})
            .await
            .unwrap()
            .unwrap();
        let journal: serde_json::Value =
            mongodb::bson::deserialize_from_bson(row.get("record").unwrap().clone()).unwrap();
        assert_eq!(
            journal["home"].as_array().unwrap().len(),
            1,
            "dispatch must be durable before creation"
        );
        let home = HomeVolumeAttachment {
            request: request.clone(),
            receipt: self.prepare(request).await?,
        };
        if self.lose_create.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "lost original create reply".into(),
            ));
        }
        Ok(serde_json::from_value(json!({"provider_scope":self.allocation_scope().unwrap(),"home":home,"operation_id":operation_id,"creation_resource_version":"10","creation_response_sha256":provider_request_digest(&home)?})).unwrap())
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        if self.stored.lock().await.get(&home.request.volume_id) != Some(home) {
            return Err(AdapterError::Rejected("home was replaced".into()));
        }
        Ok(())
    }
    async fn remove_captures(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        self.verify(home).await?;
        self.captures.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn delete_original_home(
        &self,
        original: &HomeProvisionReceipt,
        operation_id: &str,
    ) -> layer3_adapter::Result<HomeDeleteReceipt> {
        self.verify(original.home()).await?;
        assert!(
            self.captures.load(Ordering::SeqCst) > 0,
            "owned captures must be cleaned before the home claim"
        );
        self.deletes.fetch_add(1, Ordering::SeqCst);
        if self.lose_delete.swap(false, Ordering::SeqCst) {
            self.stored
                .lock()
                .await
                .remove(&original.home().request.volume_id);
            return Err(AdapterError::Unavailable(
                "lost original claim delete acknowledgement".into(),
            ));
        }
        Ok(serde_json::from_value(json!({"original":original,"operation_id":operation_id,"resource_version":"20","response_sha256":provider_request_digest(original)?})).unwrap())
    }
    async fn observe_home_finality(
        &self,
        delete: &HomeDeleteReceipt,
    ) -> layer3_adapter::Result<HomeFinalityReceipt> {
        self.verify(delete.original().home()).await?;
        if self.pending.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "original home claim deletion remains pending".into(),
            ));
        }
        self.stored
            .lock()
            .await
            .remove(&delete.original().home().request.volume_id);
        let absence =
            provider_request_digest(&("cma.namespaced-home-claim-absence.v1", delete, 404_u16))?;
        Ok(serde_json::from_value(json!({"delete":delete,"claim_absence_observation_sha256":absence,"cleanup_scope":"namespaced_claim"})).unwrap())
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("generic home removal is forbidden")
    }
}

async fn fresh(lost_create: bool) -> (RetirementFixture, Arc<Homes>) {
    let mut original = home::seed().await;
    let homes = Arc::new(Homes {
        database: original.client.database(&original.database),
        stored: Default::default(),
        pending: AtomicBool::new(true),
        lose_create: AtomicBool::new(lost_create),
        deletes: AtomicUsize::new(0),
        lose_delete: AtomicBool::new(false),
        captures: AtomicUsize::new(0),
    });
    let hosted = CodexAdapter::with_backend(original.backend.clone(), CodexSandboxSpec::default())
        .with_channel_tools(original.tools.clone())
        .with_home_volumes(homes.clone());
    original.adapter = Arc::new(fixture::RetirementAdapter::new(Arc::new(hosted)).with_finality());
    original.driver = ChannelProvisioningDriver::new(
        ChannelProvisioningDependencies {
            allocations: original.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            issuer: original.server.issuer.clone(),
            custody: original.custody.clone(),
            adapter: original.adapter.clone(),
        },
        original.settings.clone(),
        "home-finality-provisioning".into(),
    )
    .unwrap();
    let renewal = RenewalFixture::from_original(original).await;
    let registry = crate::sandbox::channel_startup::test_support::publish_connection(
        renewal.original.resources.clone(),
        renewal.original.adapter.clone(),
        &renewal.original.record.placement.target().agent_id,
    )
    .await;
    let fixture = RetirementFixture::with_agents(renewal, registry, None);
    fixture.start().await;
    (fixture, homes)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_finality_resumes_from_persisted_delete_ack_and_retires_after_claim_absence() {
    let (f, homes) = fresh(false).await;
    for _ in 0..6 {
        f.step().await;
    }
    assert_eq!(homes.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    let effects = f
        .db()
        .collection::<Document>("channel_provider_teardown_receipts")
        .find_one(doc! {"_id":&f.f.original.record.id})
        .await
        .unwrap()
        .unwrap();
    assert!(effects
        .get_document("record")
        .unwrap()
        .get_document("home_delete")
        .is_ok());
    f.step().await;
    assert_eq!(f.sandbox().await.status, SandboxStatus::Retiring);
    assert_eq!(
        f.sandbox().await.work.step,
        SandboxWorkStep::TeardownRuntime
    );
    homes.pending.store(false, Ordering::SeqCst);
    f.retry().await;
    for _ in 0..3 {
        f.step().await;
    }
    let sandbox = f.sandbox().await;
    assert_eq!(sandbox.status, SandboxStatus::Retired);
    assert!(sandbox.purge_after.is_none());
    let finality = f
        .db()
        .collection::<Document>("channel_provider_finality_receipts")
        .find_one(doc! {"_id": &f.f.original.record.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        finality
            .get_document("record")
            .unwrap()
            .get_document("home")
            .unwrap()
            .get_document("receipt")
            .unwrap()
            .get_str("cleanup_scope")
            .unwrap(),
        "namespaced_claim"
    );
    assert!(sandbox.lifecycle.home.is_none() && sandbox.runtime.is_none());
    assert_eq!(homes.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.db()
            .collection::<Document>("channel_retirement_finalizations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_finality_lost_original_creation_reply_never_authorizes_home_delete() {
    let (f, homes) = fresh(true).await;
    for _ in 0..7 {
        f.step().await;
    }
    assert_eq!(f.sandbox().await.status, SandboxStatus::Retiring);
    assert_eq!(f.sandbox().await.work.step, SandboxWorkStep::MarkRetired);
    assert_eq!(homes.deletes.load(Ordering::SeqCst), 0);
    assert!(!homes.stored.lock().await.is_empty());
    assert_eq!(
        f.db()
            .collection::<Document>("channel_provider_finality_receipts")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_finality_foreign_uid_or_lost_delete_ack_never_publishes_retired() {
    for lost_ack in [false, true] {
        let (f, homes) = fresh(false).await;
        for _ in 0..5 {
            f.step().await;
        }
        if lost_ack {
            homes.lose_delete.store(true, Ordering::SeqCst);
        } else {
            for home in homes.stored.lock().await.values_mut() {
                home.receipt.storage_generation = "foreign-claim-uid".into();
            }
        }
        f.step().await;
        assert_eq!(f.sandbox().await.status, SandboxStatus::Retiring);
        assert_eq!(
            f.sandbox().await.work.step,
            SandboxWorkStep::TeardownRuntime
        );
        let deletes = homes.deletes.load(Ordering::SeqCst);
        assert_eq!(deletes, usize::from(lost_ack));
        f.retry().await;
        f.step().await;
        assert_eq!(homes.deletes.load(Ordering::SeqCst), deletes);
        assert_eq!(
            f.db()
                .collection::<Document>("channel_retirement_finalizations")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        let effects = f
            .db()
            .collection::<Document>("channel_provider_teardown_receipts")
            .find_one(doc! {"_id": &f.f.original.record.id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            effects.get_document("record").unwrap().get("home_delete"),
            Some(&mongodb::bson::Bson::Null)
        );
        f.close().await;
    }
}
