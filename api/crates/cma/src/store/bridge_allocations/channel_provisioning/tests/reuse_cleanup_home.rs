//! A disposable in-memory volume store; runtime status still comes from the adapter.
use super::fixture::{Fixture, VerifiedHuman};
use crate::sandbox::channel_provisioning::{
    ChannelProvisioningDependencies, ChannelProvisioningDriver,
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeStore,
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Default)]
struct Storage(Mutex<BTreeMap<String, HomeVolumeAttachment>>);

#[async_trait::async_trait]
impl HomeVolumeStore for Storage {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        Some(
            layer3_adapter::HomeVolumeScope::from_authority(
                "in-memory-fixture",
                &["channel-home-fixture"],
            )
            .unwrap(),
        )
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.0.lock().await.values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        request.validate()?;
        let mut rows = self.0.lock().await;
        let stored =
            rows.entry(request.volume_id.clone())
                .or_insert_with(|| HomeVolumeAttachment {
                    request: request.clone(),
                    receipt: HomeVolumeReceipt {
                        volume_id: request.volume_id.clone(),
                        request_sha256: request.computed_sha256(),
                        storage_reference: format!("home-{}", &request.computed_sha256()[..40]),
                        storage_generation: "cleanup-volume-generation".into(),
                    },
                });
        assert_eq!(&stored.request, request);
        stored.receipt.validate_for(request)?;
        Ok(stored.receipt.clone())
    }
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        attachment.validate_for(&attachment.request.sandbox_id)?;
        assert_eq!(
            self.0.lock().await.get(&attachment.request.volume_id),
            Some(attachment)
        );
        Ok(())
    }
    async fn remove(
        &self,
        _request: &HomeVolumeRequest,
        _receipt: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("cleanup requests must never delete the shared parent home")
    }
}

pub(super) async fn seed() -> Fixture {
    let mut seed = Fixture::new().await;
    seed.settings.home_policy = Some(HomeVolumePolicy {
        storage_class: "cma-home-v1".into(),
        capacity_gib: 64,
        template_name: "seed".into(),
        template_generation: "cleanup-seed".into(),
    });
    seed.adapter = Arc::new(
        CodexAdapter::with_backend(seed.backend.clone(), CodexSandboxSpec::default())
            .with_channel_tools(seed.tools.clone())
            .with_home_volumes(Arc::new(Storage::default())),
    );
    seed.driver = ChannelProvisioningDriver::new(
        ChannelProvisioningDependencies {
            allocations: seed.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            issuer: seed.server.issuer.clone(),
            custody: seed.custody.clone(),
            adapter: seed.adapter.clone(),
        },
        seed.settings.clone(),
        "cleanup-seed-worker".into(),
    )
    .unwrap();
    seed
}
