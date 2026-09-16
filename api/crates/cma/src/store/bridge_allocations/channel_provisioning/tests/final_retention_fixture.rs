use super::*;
use crate::{
    bridge::provisioning::retirement::ChannelRetirementAuthority,
    sandbox::channel_retirement::model::ChannelRetirementSnapshot,
    store::bridge_allocations::channel_retirement::RetirementChange,
};
use layer2_core::sandbox::store::SandboxWorkerStore;
use layer3_adapter::{HomeVolumeAttachment, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeStore};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        Arc,
    },
};

pub(super) struct Homes {
    database: mongodb::Database,
    pub observed: tokio::sync::Mutex<BTreeMap<String, HomeVolumeAttachment>>,
    pub prepares: AtomicUsize,
    pub reads: AtomicUsize,
    lost_response: AtomicBool,
}

#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        Some(
            layer3_adapter::HomeVolumeScope::from_authority("fixture", &["final-retention"])
                .unwrap(),
        )
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.observed.lock().await.values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        let row = self
            .database
            .collection::<Document>("sandboxes")
            .find_one(doc! {"_id":&request.sandbox_id})
            .await
            .unwrap()
            .unwrap();
        let original: ChannelHomeProvenance = mongodb::bson::deserialize_from_bson(
            row.get("channel_home_provenance").unwrap().clone(),
        )
        .unwrap();
        let evidence: ChannelHomePrepareEvidence = mongodb::bson::deserialize_from_bson(
            row.get("channel_home_prepare_evidence").unwrap().clone(),
        )
        .unwrap();
        assert_eq!(original.request, *request);
        assert_eq!(
            evidence,
            ChannelHomePrepareEvidence::capture(&original).unwrap()
        );
        self.prepares.fetch_add(1, Ordering::SeqCst);
        let mut observed = self.observed.lock().await;
        let receipt = observed
            .entry(request.volume_id.clone())
            .or_insert_with(|| HomeVolumeAttachment {
                request: request.clone(),
                receipt: HomeVolumeReceipt {
                    volume_id: request.volume_id.clone(),
                    request_sha256: request.computed_sha256(),
                    storage_reference: format!("home-{}", &request.computed_sha256()[..40]),
                    storage_generation: "original-fixture-generation".into(),
                },
            })
            .receipt
            .clone();
        if self.lost_response.swap(false, Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Unavailable(
                "lost fixture prepare reply".into(),
            ));
        }
        Ok(receipt)
    }
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            self.observed
                .lock()
                .await
                .get(&attachment.request.volume_id),
            Some(attachment)
        );
        Ok(())
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("retained uncertainty must never remove physical storage")
    }
}

pub(super) async fn lost_prepare_home() -> (RetirementFixture, Arc<Homes>) {
    use crate::sandbox::channel_provisioning::{
        ChannelProvisioningDependencies, ChannelProvisioningDriver,
    };
    use crate::store::bridge_definitions::mongo_integration_tests::allocations::channel_provisioning::{
        fixture::VerifiedHuman, home,
    };
    use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
    let mut original = home::seed().await;
    let homes = Arc::new(Homes {
        database: original.client.database(&original.database),
        observed: Default::default(),
        prepares: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        lost_response: AtomicBool::new(true),
    });
    original.adapter = Arc::new(
        CodexAdapter::with_backend(original.backend.clone(), CodexSandboxSpec::default())
            .with_channel_tools(original.tools.clone())
            .with_home_volumes(homes.clone()),
    );
    original.driver = ChannelProvisioningDriver::new(
        ChannelProvisioningDependencies {
            allocations: original.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            issuer: original.server.issuer.clone(),
            custody: original.custody.clone(),
            adapter: original.adapter.clone(),
        },
        original.settings.clone(),
        "final-retention-provisioning".into(),
    )
    .unwrap();
    original.step().await.unwrap();
    assert!(matches!(
        original.step().await,
        Err(AllocationError::Unavailable)
    ));
    original.release_current_claim().await;
    let f = RetirementFixture::from_renewal(RenewalFixture::from_original(original).await);
    f.start().await;
    for _ in 0..6 {
        f.step().await;
    }
    (f, homes)
}

pub(super) async fn document(f: &RetirementFixture) -> Document {
    f.db()
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":f.f.original.record.placement.target().sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn set(f: &RetirementFixture, fields: Document) {
    f.db()
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":f.f.original.record.placement.target().sandbox_id.as_str()},
            doc! {"$set":fields},
        )
        .await
        .unwrap();
}

pub(super) async fn unset(f: &RetirementFixture, fields: Document) {
    f.db()
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":f.f.original.record.placement.target().sandbox_id.as_str()},
            doc! {"$unset":fields},
        )
        .await
        .unwrap();
}

pub(super) async fn read(
    f: &RetirementFixture,
) -> Result<ChannelRetirementSnapshot, AllocationError> {
    f.retry().await;
    let now = SystemTime::now();
    let claimed =
        f.f.original
            .resources
            .claim_sandbox_work(
                &f.f.original.record.placement.target().sandbox_id,
                "final-retention-assessment",
                layer2_core::rfc3339_at(now),
                layer2_core::rfc3339_at(now + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
    let permit = ChannelRetirementAuthority::fixture(
        f.f.original.record.clone(),
        f.f.original.authority.clone(),
        f.f.original.lease.clone(),
        claimed.clone(),
    );
    let result =
        f.f.original
            .allocations
            .change_channel_retirement(&permit, &f.f.original.custody, RetirementChange::Read, now)
            .await;
    assert_eq!(
        f.sandbox().await,
        claimed,
        "assessment cannot change canonical state"
    );
    result
}

pub(super) async fn assessed(f: &RetirementFixture) -> FinalRetentionAssessment {
    FinalRetentionAssessment::capture(&read(f).await.unwrap()).unwrap()
}

pub(super) async fn retained_home() -> RetirementFixture {
    let f = with_persistent_home().await;
    f.step().await;
    assert!(retention(&f).await.is_some());
    f
}

pub(super) async fn assert_preserved(
    f: &RetirementFixture,
    original: &Document,
    audit: &[Document],
) {
    let after = document(f).await;
    for field in [
        "channel_home_provenance",
        "channel_home_prepare_evidence",
        "channel_retirement",
        "channel_storage_retention",
    ] {
        assert_eq!(after.get(field), original.get(field), "{field}");
    }
    assert_eq!(rows(f, "channel_retirement_audits").await, audit);
    let sandbox = f.sandbox().await;
    assert_eq!(sandbox.status, SandboxStatus::Retiring);
    assert_eq!(sandbox.work.step, SandboxWorkStep::MarkRetired);
    assert!(sandbox.retired_at.is_none() && sandbox.purge_after.is_none());
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert!(
        f.f.original
            .allocations
            .channel_allocation(&f.f.original.record.id)
            .await
            .unwrap()
            == Some(f.f.original.record.clone())
    );
}
