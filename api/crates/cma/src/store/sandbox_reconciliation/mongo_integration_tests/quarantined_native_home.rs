use super::*;
use crate::store::{
    runtime_routing::MongoRuntimeRouting,
    sandbox_mongo::{commit_transaction, CommitOutcome},
    sandbox_mongo_integration_tests::native_home_fixture::native_home,
    sandbox_quarantine::FIELD,
    work_schema,
};
use layer3_adapter::agent_sandbox::{
    NativePoolBinding, NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup,
    NativePoolConfiguration, NativePoolControl, NativePoolSpec, NativePoolStatus,
};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{
    AgentLaunch, ConversationChannel, ConversationUpdate, Result, RuntimeHandle, RuntimeObservation,
};

struct Provider {
    claims: Mutex<Vec<NativePoolClaim>>,
    runtimes: Mutex<Vec<RuntimeObservation>>,
}

#[async_trait::async_trait]
impl HostedBackend for Provider {
    fn native_pools(&self) -> Option<&dyn NativePoolControl> {
        Some(self)
    }
    fn native_pools_for_home(
        &self,
        home: &HomeVolumeRequest,
    ) -> Result<Option<&dyn NativePoolControl>> {
        Ok(home.policy.is_native_vct().then_some(self))
    }
    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        Ok(self.runtimes.lock().await.clone())
    }
    async fn reclaim(&self, observed: &RuntimeObservation) -> Result<()> {
        self.runtimes
            .lock()
            .await
            .retain(|runtime| runtime != observed);
        Ok(())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        panic!("startup does not provision runtimes")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("the fixture has no live process")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("startup does not open Agents")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("the fixture has no live process")
    }
}

#[async_trait::async_trait]
impl NativePoolControl for Provider {
    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        Ok(self.claims.lock().await.clone())
    }
    async fn reclaim_claim(&self, claim: &NativePoolClaim) -> Result<()> {
        self.claims
            .lock()
            .await
            .retain(|retained| retained != claim);
        Ok(())
    }
    async fn cleanup_retained_home(&self, _: &HomeVolumeAttachment) -> Result<()> {
        panic!("the quarantined native home must stay in custody")
    }
    fn plan(&self, _: &NativePoolSpec) -> Result<NativePoolConfiguration> {
        panic!("startup does not plan pool capacity")
    }
    async fn configure_planned(
        &self,
        _: &NativePoolConfiguration,
        _: u32,
    ) -> Result<NativePoolStatus> {
        panic!("startup does not configure pools in this fixture")
    }
    async fn inventory(&self) -> Result<Vec<NativePoolStatus>> {
        Ok(Vec::new())
    }
    async fn resize(&self, _: &NativePoolBinding, _: u32) -> Result<NativePoolStatus> {
        panic!("startup does not resize pools in this fixture")
    }
    async fn claim(&self, _: &NativePoolClaimRequest) -> Result<NativePoolClaim> {
        panic!("startup does not allocate claims")
    }
    async fn observe_claim(&self, claim: &NativePoolClaim) -> Result<NativePoolClaim> {
        Ok(claim.clone())
    }
    async fn adopt_home(&self, _: &NativePoolClaim) -> Result<HomeVolumeAttachment> {
        panic!("startup does not adopt a quarantined home")
    }
    async fn plan_cleanup(&self, _: &NativePoolCleanup) -> Result<NativePoolCleanup> {
        panic!("startup reaches cleanup through the original claim")
    }
    async fn cleanup(&self, _: &NativePoolCleanup) -> Result<()> {
        panic!("startup reaches cleanup through the original claim")
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_sandbox_quarantine_preserves_native_claim_and_home_through_startup() {
    let (client, name, store) = database().await;
    let db = client.database(&name);
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    work_schema::floor::retain(&store, &mut session)
        .await
        .unwrap();
    work_schema::floor::retain_managed_machine_origin(&store, &mut session)
        .await
        .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));

    let mut sandbox =
        crate::store::sandbox_mongo_integration_tests::sandbox("sbx_quarantined_home");
    let retained = native_home(&sandbox.id);
    let attachment = retained.attachment().unwrap();
    let claim = retained
        .native_pool
        .as_ref()
        .unwrap()
        .claim
        .as_ref()
        .unwrap()
        .clone();
    sandbox.lifecycle.home = Some(Box::new(retained));
    sandbox.status = SandboxStatus::Paused;
    let mut raw = sandbox_document(&sandbox, 0).unwrap();
    raw.insert("document_schema_version", work_schema::RENEWED_VERSION);
    let record = raw.get_document_mut("record").unwrap();
    record
        .get_document_mut("lifecycle")
        .unwrap()
        .insert("runtime_image_origin", "managed_machine");
    record
        .get_document_mut("work")
        .unwrap()
        .insert("authority_protocol", "renewed_v1");
    assert!(
        matches!(read_sandbox(&raw), Err(SandboxStoreError::Corrupt(reason))
        if reason == "runtime image origin disagrees with ordinary-work envelope")
    );
    store.sandboxes.insert_one(raw.clone()).await.unwrap();

    let homes = Arc::new(Homes::default());
    homes
        .stored
        .lock()
        .await
        .insert(attachment.request.volume_id.clone(), attachment.clone());
    let orphan = home(&homes, &SandboxId("sbx_unrelated_orphan".into())).await;
    let mut orphan_claim = native_home(&SandboxId("sbx_orphan_claim".into()))
        .native_pool
        .unwrap()
        .claim
        .unwrap();
    orphan_claim.uid = "unrelated-claim-uid".into();
    let retained_runtime = RuntimeObservation {
        sandbox_id: sandbox.id.to_string(),
        generation: 1,
        handle: RuntimeHandle("agent-sandbox:member:sandbox-uid".into()),
        image_sha256: "a".repeat(43),
        spec_fingerprint: "b".repeat(43),
        runtime_contract: "3".into(),
        lifecycle: layer3_adapter::RuntimeLifecycle {
            state: "stopped".into(),
            reason: None,
        },
    };
    let orphan_runtime = RuntimeObservation {
        sandbox_id: "sbx_orphan_runtime".into(),
        handle: RuntimeHandle("agent-sandbox:orphan:orphan-uid".into()),
        ..retained_runtime.clone()
    };
    let provider = Arc::new(Provider {
        claims: Mutex::new(vec![claim.clone(), orphan_claim]),
        runtimes: Mutex::new(vec![retained_runtime.clone(), orphan_runtime]),
    });
    let adapter = CodexAdapter::new(provider.clone(), CodexSandboxSpec::default())
        .with_home_volumes(homes.clone());
    let reconciler = SandboxReconciler::new(
        Arc::new(store.clone()),
        Arc::new(adapter),
        Arc::new(SandboxPolicy::default()),
        "fixture-quarantine".into(),
    );

    MongoRuntimeRouting::open(&db, "fixture-quarantine", false)
        .await
        .unwrap();
    reconciler
        .initialize_before_workers(SystemTime::now())
        .await
        .unwrap();

    let mut stored = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(stored.remove(FIELD).is_some());
    assert_eq!(stored.to_vec().unwrap(), raw.to_vec().unwrap());
    assert_eq!(provider.inventory_claims().await.unwrap(), [claim]);
    assert_eq!(
        HostedBackend::inventory(provider.as_ref()).await.unwrap(),
        [retained_runtime]
    );
    assert_eq!(homes.inventory().await.unwrap(), [attachment]);
    assert_eq!(*homes.removed.lock().await, [orphan.attachment().unwrap()]);
    db.drop().await.unwrap();
}
