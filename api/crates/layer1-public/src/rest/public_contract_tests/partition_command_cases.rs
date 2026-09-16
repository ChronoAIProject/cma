//! Exact recovered receipts through the real authenticated REST service.
//! Atomic effects and first acceptance are covered by the disposable Mongo tests.
use super::super::{bridge_cleanup_tests::fixture, bridges_tests::fixture::Ports};
use super::Scenario;
use axum::Extension;
use layer2_core::bridge::{allocation::AllocationState, partition_commands::*, *};
use std::sync::{atomic::AtomicUsize, Arc};

struct Retained(PartitionCommandReceipt);
#[async_trait::async_trait]
impl BridgePartitionCommandStore for Retained {
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
    ) -> Result<Option<PartitionCommandReceipt>, PartitionCommandError> {
        manager.require_current()?;
        if self.0.bridge_id != manager.record().id
            || &self.0.actor != manager.actor()
            || self.0.request_sha256 != digest
        {
            return Err(PartitionCommandError::Conflict);
        }
        let mut receipt = self.0.clone();
        receipt.id = id.into();
        Ok(Some(receipt))
    }
    async fn snapshot(
        &self,
        _: &BridgeManagementLease,
        _: &str,
    ) -> Result<PartitionCommandSnapshot, PartitionCommandError> {
        Err(PartitionCommandError::NotFound)
    }
    async fn apply(
        &self,
        _: PartitionCommandWrite,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError> {
        Err(PartitionCommandError::Corrupt)
    }
}
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let action = match name {
        "partition_command_reset_receipt" => PartitionCommandAction::Reset,
        "partition_command_detach_receipt" => PartitionCommandAction::Detach,
        _ => return None,
    };
    let base = fixture::fixture().await;
    let record = &base.memory.records[0];
    let ports = Arc::new(Ports {
        role: AtomicUsize::new(2),
        calls: AtomicUsize::new(0),
        validation: AtomicUsize::new(1),
    });
    let definitions =
        BridgeService::new(Arc::new(MemoryBridgeStore::default()), ports.clone(), ports);
    let context = fixture::context("owner-a");
    definitions
        .mutate(
            &context,
            Some("request-credential"),
            BridgeMutation {
                id: base.bridge.clone(),
                expected_revision: 0,
                idempotency_key: "partition-contract-definition".into(),
                change: BridgeChange::Create {
                    owner: record.owner.clone(),
                    config: Box::new(record.config.clone()),
                },
            },
        )
        .await
        .unwrap();
    let allocation_id = format!("bral_{}", "1".repeat(64));
    let request = PartitionCommandRequest {
        allocation_id: allocation_id.clone(),
        action,
        expected_bridge_revision: 1,
        expected_revision: 4,
    };
    let receipt = PartitionCommandReceipt {
        id: format!("bpc_{}", "3".repeat(64)),
        bridge_id: base.bridge.clone(),
        request_sha256: command_hash(&request).unwrap(),
        actor: context.actor.clone(),
        action,
        expected_bridge_revision: 1,
        allocation_id: allocation_id.clone(),
        generation: 1,
        previous_revision: 4,
        result: PartitionCommandResult {
            id: allocation_id,
            generation: 1,
            revision: 5,
            state: AllocationState::Retired,
        },
        successor: (action == PartitionCommandAction::Reset).then(|| PartitionCommandResult {
            id: format!("bral_{}", "2".repeat(64)),
            generation: 2,
            revision: 1,
            state: AllocationState::Reserved,
        }),
        created_at_ms: 1000,
    };
    let service = BridgePartitionCommandService::new(definitions, Arc::new(Retained(receipt)));
    Some(Scenario {
        router: super::super::bridge_partition_commands_router(Some(Arc::new(service)))
            .layer(Extension(context.clone()))
            .layer(Extension(crate::DelegatedCredential::new(
                "request-credential".into(),
            ))),
        subject: Some("owner-a"),
        authorization: context.caller.authorization,
        symbols: Default::default(),
    })
}
