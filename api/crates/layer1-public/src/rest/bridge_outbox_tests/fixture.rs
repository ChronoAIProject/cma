use super::super::{bridge_invokes_tests::fixture as invokes, resource_access_tests::context};
use layer2_core::bridge::{
    outbox::{management::*, *},
    *,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Memory {
    state: Mutex<(
        BTreeMap<String, OutboxItem>,
        BTreeMap<String, OutboxRecoveryReceipt>,
    )>,
}
#[async_trait::async_trait]
impl BridgeOutboxManagementStore for Memory {
    async fn list(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<OutboxItem>, OutboxManagementError> {
        manager.require_current()?;
        let state = self.state.lock().unwrap();
        state
            .0
            .values()
            .filter(|item| {
                item.source.pin().bridge_id == manager.record().id
                    && after.is_none_or(|id| id < item.id.as_str())
            })
            .take(limit as usize + 1)
            .map(|item| {
                require_source(manager, item)?;
                Ok(item.clone())
            })
            .collect()
    }
    async fn item(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<OutboxItem>, OutboxManagementError> {
        manager.require_current()?;
        let value = self.state.lock().unwrap().0.get(id).cloned();
        if let Some(item) = &value {
            require_source(manager, item)?;
        }
        Ok(value)
    }
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
    ) -> Result<Option<OutboxRecoveryReceipt>, OutboxManagementError> {
        manager.require_current()?;
        let receipt = self.state.lock().unwrap().1.get(id).cloned();
        if let Some(receipt) = &receipt {
            if receipt.bridge_id != manager.record().id || receipt.actor != *manager.actor() {
                return Err(OutboxManagementError::NotFound);
            }
            if receipt.request_sha256 != digest {
                return Err(BridgeError::IdempotencyConflict.into());
            }
        }
        Ok(receipt)
    }
    async fn apply(
        &self,
        write: OutboxRecoveryWrite,
    ) -> Result<OutboxRecoveryReceipt, OutboxManagementError> {
        write.require_current()?;
        if let Some(receipt) = self
            .recover(
                write.management(),
                write.receipt_id(),
                write.request_sha256(),
            )
            .await?
        {
            return Ok(receipt);
        }
        let mut state = self.state.lock().unwrap();
        let item = state
            .0
            .get_mut(&write.request().item_id)
            .ok_or(OutboxManagementError::NotFound)?;
        write.apply(item, 10_000)?;
        let receipt = OutboxRecoveryReceipt {
            id: write.receipt_id().into(),
            bridge_id: write.management().record().id.clone(),
            request_sha256: write.request_sha256().into(),
            actor: write.management().actor().clone(),
            action: write.request().action,
            result: (&*item).into(),
            created_at_ms: 10_000,
        };
        state.1.insert(receipt.id.clone(), receipt.clone());
        Ok(receipt)
    }
}

pub(in crate::rest) struct Fixture {
    pub router: axum::Router,
    pub id: String,
    pub item: OutboxItem,
}
pub(in crate::rest) async fn fixture() -> Fixture {
    let f = invokes::fixture().await;
    f.definitions
        .mutate(
            &context("owner-a"),
            None,
            BridgeMutation {
                id: BridgeId(f.id.clone()),
                expected_revision: 1,
                idempotency_key: "outbox-drain".into(),
                change: BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            },
        )
        .await
        .unwrap();
    let bridge = f
        .bridge_store
        .bridge(&BridgeId(f.id.clone()))
        .await
        .unwrap()
        .unwrap();
    let config = &bridge.current().unwrap().config;
    let BridgePlacementPolicy::ExistingAgent { target } = &config.placement else {
        panic!("retained target fixture");
    };
    // Historical fixture only: no fabricated active Channel session permission.
    let source:OutboxSource=serde_json::from_value(json!({"inbox_id":"1".repeat(64),"inbox_sha256":"2".repeat(64),"allocation_id":"allocation",
        "command_id":"command-outbox","response_id":"response-outbox","admission_sha256":"3".repeat(64),"owner":bridge.owner,
        "pin":{"bridge_id":bridge.id,"config_revision":1},"config":config,"target":target,"message_id":"original-inbound","chat_id":"protected-original-chat",
        "source_sha256":"4".repeat(64),"reply_expires_at_ms":null,"route_expires_at_ms":null})).unwrap();
    let invocation = format!("brti_{}", "5".repeat(64));
    let operation = RelayOperation::InitialReply;
    let id = format!(
        "bro_{}",
        layer2_core::profiles::content_digest(&(
            "cma.bridge.outbox.v1",
            source.fingerprint().unwrap(),
            &invocation,
            1_u64,
            0_u32,
            &operation
        ))
        .unwrap()
    );
    let mut item:OutboxItem=serde_json::from_value(json!({"id":id,"source":source,"tool_invocation_id":invocation,"turn_sequence":1,"sequence":1,"chunk_index":0,"chunk_count":1,"last_chunk":true,
        "predecessor":null,"content_sha256":layer2_core::profiles::content_digest(&"Private reply text").unwrap(),"text":"Private reply text",
        "credential":{"kind":"retained_key","reference":config.connector.custody,"revision":1,"key_state_version":3},"operation":operation,"revision":1,"state":{"kind":"pending"},"attempts":[],"duplicate_risk":false,"created_at_ms":1_000,"updated_at_ms":1_000})).unwrap();
    let attempt = item.claim("worker".into(), 2_000, 3_000, None).unwrap();
    item.begin_send("worker", &attempt, 2_000).unwrap();
    item.recover_expired(3_000).unwrap();
    let memory = Arc::new(Memory::default());
    memory
        .state
        .lock()
        .unwrap()
        .0
        .insert(item.id.clone(), item.clone());
    Fixture {
        router: super::super::bridge_outbox_router(Some(Arc::new(BridgeOutboxService::new(
            f.definitions,
            memory,
        )))),
        id: f.id,
        item,
    }
}
