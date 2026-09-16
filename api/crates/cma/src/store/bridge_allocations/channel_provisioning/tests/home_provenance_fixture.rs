use super::super::{
    fixture::{Fixture, VerifiedHuman},
    home,
};
use crate::sandbox::channel_provisioning::{
    home_prepare_evidence::ChannelHomePrepareEvidence, home_provenance::ChannelHomeProvenance,
    ChannelProvisioningDependencies, ChannelProvisioningDriver,
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    AdapterError, HomeVolumeAttachment, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope,
    HomeVolumeStore,
};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub(super) struct Homes {
    database: Database,
    pub scope: Mutex<HomeVolumeScope>,
    pub requests: tokio::sync::Mutex<Vec<HomeVolumeRequest>>,
    pub stored: tokio::sync::Mutex<BTreeMap<String, HomeVolumeAttachment>>,
    lost_response: AtomicBool,
}

#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(self.scope.lock().unwrap().clone())
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.stored.lock().await.values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        // Assert durable admission before the actual fixture storage effect.
        let row = self
            .database
            .collection::<Document>("sandboxes")
            .find_one(doc! {"_id":&request.sandbox_id})
            .await
            .unwrap()
            .unwrap();
        if let Some(provenance) = provenance(&row) {
            assert_eq!(provenance.request, *request);
            assert_eq!(Some(provenance.scope.clone()), self.allocation_scope());
            assert_eq!(
                prepare_evidence(&row),
                Some(ChannelHomePrepareEvidence::capture(&provenance).unwrap()),
                "dispatch uncertainty must commit before the provider effect"
            );
        }
        let canonical = crate::store::sandboxes::read_sandbox(&row).unwrap();
        assert_eq!(&canonical.lifecycle.home.as_ref().unwrap().request, request);
        self.requests.lock().await.push(request.clone());
        let mut stored = self.stored.lock().await;
        let home =
            stored
                .entry(request.volume_id.clone())
                .or_insert_with(|| HomeVolumeAttachment {
                    request: request.clone(),
                    receipt: HomeVolumeReceipt {
                        volume_id: request.volume_id.clone(),
                        request_sha256: request.computed_sha256(),
                        storage_reference: format!("home-{}", &request.computed_sha256()[..40]),
                        storage_generation: "original-generation".into(),
                    },
                });
        assert_eq!(home.request, *request);
        let receipt = home.receipt.clone();
        if self.lost_response.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fixture lost prepare response".into(),
            ));
        }
        Ok(receipt)
    }
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        if self.stored.lock().await.get(&attachment.request.volume_id) != Some(attachment) {
            return Err(AdapterError::Rejected(
                "fixture home generation changed".into(),
            ));
        }
        Ok(())
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("provenance never authorizes home removal")
    }
}

pub(super) fn reopen(f: &mut Fixture, homes: Arc<Homes>) {
    f.adapter = Arc::new(
        CodexAdapter::with_backend(f.backend.clone(), CodexSandboxSpec::default())
            .with_channel_tools(f.tools.clone())
            .with_home_volumes(homes),
    );
    f.driver = ChannelProvisioningDriver::new(
        ChannelProvisioningDependencies {
            allocations: f.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            issuer: f.server.issuer.clone(),
            custody: f.custody.clone(),
            adapter: f.adapter.clone(),
        },
        f.settings.clone(),
        "home-provenance-worker".into(),
    )
    .unwrap();
}

pub(super) async fn fixture(lost_response: bool) -> (Fixture, Arc<Homes>) {
    let mut f = home::seed().await;
    let homes = Arc::new(Homes {
        database: f.client.database(&f.database),
        scope: Mutex::new(
            HomeVolumeScope::from_authority("in-memory-fixture", &["original-scope"]).unwrap(),
        ),
        requests: Default::default(),
        stored: Default::default(),
        lost_response: AtomicBool::new(lost_response),
    });
    reopen(&mut f, homes.clone());
    (f, homes)
}

pub(super) async fn row(f: &Fixture) -> Document {
    f.client
        .database(&f.database)
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":f.record.placement.target().sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap()
}

pub(super) fn provenance(row: &Document) -> Option<ChannelHomeProvenance> {
    row.get("channel_home_provenance")
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()).unwrap())
}

pub(super) fn prepare_evidence(row: &Document) -> Option<ChannelHomePrepareEvidence> {
    row.get("channel_home_prepare_evidence")
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()).unwrap())
}
