use super::execution_tests::execution;
use super::*;
use crate::store::bridge_definitions::inbox::receipts::model::{
    AcceptanceDisposition, AcceptedCallbackReceipt,
};
use layer2_core::bridge::{
    BridgeConfig, BridgeConnectionValidator, BridgeError, BridgeFeature, BridgePlatform,
    BridgeValidation,
};
use std::sync::atomic::{AtomicBool, Ordering};

struct ExecutionGate {
    enabled: AtomicBool,
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for ExecutionGate {
    fn check_feature(
        &self,
        feature: BridgeFeature,
        platform: &BridgePlatform,
    ) -> Result<(), BridgeError> {
        assert_eq!(feature, BridgeFeature::Execution);
        assert_eq!(*platform, BridgePlatform::Telegram);
        if self.enabled.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(BridgeError::Unavailable)
        }
    }
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        panic!("Feature availability must not replace the real authority verifier")
    }
}

async fn admit(
    c: &ChannelFixture,
    service: &ChannelAgentCommandService,
) -> Result<AdmittedAgentCommand, ChannelCommandAdmissionError> {
    service
        .admit(
            c.request().await.proof().clone(),
            c.inbox.clone(),
            c.allocation.clone(),
        )
        .await
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn disabled_execution_blocks_fresh_admission_recovers_receipts_and_drains_released_fifo_once()
{
    let c = ChannelFixture::new().await;
    let gate = Arc::new(ExecutionGate {
        enabled: AtomicBool::new(false),
    });
    let service = Arc::new(
        ChannelAgentCommandService::new(c.commands.clone()).with_feature_gates(gate.clone()),
    );
    let db = c.f.client.database(&c.f.database);
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let original = receipts
        .find_one(doc! {"_id":&c.inbox.id})
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        admit(&c, &service).await,
        Err(ChannelCommandAdmissionError::Unavailable)
    ));
    assert_eq!(
        c.f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        receipts.find_one(doc! {"_id":&c.inbox.id}).await.unwrap(),
        Some(original)
    );

    gate.enabled.store(true, Ordering::SeqCst);
    let held = admit(&c, &service).await.unwrap();
    assert_eq!(
        held.receipt.agent_order, 1,
        "disabled admission must not consume shared FIFO order"
    );
    let permanent = receipts
        .find_one(doc! {"_id":&c.inbox.id})
        .await
        .unwrap()
        .unwrap();
    let receipt = AcceptedCallbackReceipt::read(&permanent).unwrap();
    let AcceptanceDisposition::Command { correlation } = receipt.disposition else {
        panic!("admission must bind permanent command evidence atomically")
    };
    assert_eq!(
        correlation.command_id,
        held.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        correlation.response_id,
        held.receipt.response_id.as_ref().unwrap().as_str()
    );
    assert_eq!(correlation.allocation_id, c.allocation.id);

    let (mut executor, sends) = execution(&c, false);
    Arc::get_mut(&mut executor).unwrap().channel = Some(service.clone());
    gate.enabled.store(false, Ordering::SeqCst);
    assert_eq!(admit(&c, &service).await.unwrap(), held);
    AgentCommandWorker::new(c.f.resources.clone(), executor.clone(), "flags-held".into())
        .unwrap()
        .run_one(&held.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(held.clone())
    );

    gate.enabled.store(true, Ordering::SeqCst);
    let authority = service
        .authorize_execution(&c.f.service, &held)
        .await
        .unwrap();
    service
        .release_execution_hold(&authority, &held)
        .await
        .unwrap();
    gate.enabled.store(false, Ordering::SeqCst);
    // Released work uses its original archive even after callback TTL cleanup;
    // feature availability must not make the shared FIFO head retry forever.
    db.collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id":&c.inbox.id})
        .await
        .unwrap();
    for holder in ["flags-drain", "flags-restarted"] {
        AgentCommandWorker::new(c.f.resources.clone(), executor.clone(), holder.into())
            .unwrap()
            .run_one(&held.receipt.envelope.command_id)
            .await
            .unwrap();
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let current =
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.receipt, held.receipt);
    assert_eq!(current.status, AgentResponseStatus::InProgress);
    assert_eq!(
        receipts.find_one(doc! {"_id":&c.inbox.id}).await.unwrap(),
        Some(permanent)
    );
    c.close().await;
}
