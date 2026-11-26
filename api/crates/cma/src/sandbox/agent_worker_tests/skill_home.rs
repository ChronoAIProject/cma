//! Exact persistent-home authority for live skill capability tests.
use super::*;
use layer2_core::sandbox::SandboxHome;
use layer3_adapter::{
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeStore,
};
struct Home(HomeVolumeAttachment);
#[async_trait::async_trait]
impl HomeVolumeStore for Home {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(vec![self.0.clone()])
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("home already allocated")
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        assert_eq!(home, &self.0);
        Ok(())
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("skill work cannot remove home storage")
    }
}
pub(super) fn attach(adapter: CodexAdapter, sandbox: &mut Sandbox) -> CodexAdapter {
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 64,
            template_name: "fixture-seed".into(),
            template_generation: "generation-one".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-pvc".into(),
        storage_generation: "original-disk".into(),
    })
    .unwrap();
    let adapter = with_bound_home(adapter, &home);
    sandbox.lifecycle.home = Some(Box::new(home));
    adapter
}

pub(super) fn with_bound_home(adapter: CodexAdapter, home: &SandboxHome) -> CodexAdapter {
    adapter.with_home_volumes(Arc::new(Home(home.attachment().unwrap())))
}
