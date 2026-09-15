use super::super::{bridge_invokes_tests::fixture as invokes, resource_access_tests::context};
use axum::{http::Method, Router};
use layer2_core::bridge::{invoke::*, *};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    policy: Option<SourceControlPolicy>,
    receipts: BTreeMap<String, SourceControlReceipt>,
    audits: BTreeMap<u64, SourceControlPolicy>,
}
#[derive(Default)]
struct Memory(Mutex<State>);
#[async_trait::async_trait]
impl BridgeSourceControlStore for Memory {
    async fn source_control_policy(
        &self,
        access: &SourceControlAccess,
    ) -> Result<Option<SourceControlPolicy>, BridgeInvokeError> {
        access.require_current()?;
        Ok(self.0.lock().unwrap().policy.clone())
    }
    async fn recover_source_control(
        &self,
        access: &SourceControlAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<SourceControlReceipt>, BridgeInvokeError> {
        access.require_current()?;
        let state = self.0.lock().unwrap();
        let Some(receipt) = state.receipts.get(id) else {
            return Ok(None);
        };
        access.require_receipt(receipt)?;
        if source_control_request_digest(&receipt.request)? != digest {
            return Err(BridgeInvokeError::IdempotencyConflict);
        }
        receipt.require_policy(
            state
                .audits
                .get(&receipt.result.revision)
                .ok_or(BridgeInvokeError::Corrupt)?,
        )?;
        Ok(Some(receipt.clone()))
    }
    async fn apply_source_control(
        &self,
        write: SourceControlWrite,
    ) -> Result<SourceControlReceipt, BridgeInvokeError> {
        write.require_current()?;
        let mut state = self.0.lock().unwrap();
        if let Some(prior) = state.receipts.get(&write.receipt().id) {
            if prior.request != write.receipt().request {
                return Err(BridgeInvokeError::IdempotencyConflict);
            }
            return Ok(prior.clone());
        }
        if state.policy.as_ref() != write.previous() {
            return Err(BridgeInvokeError::Conflict);
        }
        state.policy = Some(write.policy().clone());
        state
            .audits
            .insert(write.policy().view.revision, write.policy().clone());
        state
            .receipts
            .insert(write.receipt().id.clone(), write.receipt().clone());
        Ok(write.receipt().clone())
    }
}
pub(in crate::rest) struct Fixture {
    pub router: Router,
    pub path: String,
    pub grant: String,
    pub bridge: invokes::Fixture,
}
pub(in crate::rest) async fn fixture() -> Fixture {
    let f = invokes::fixture_with_store(Arc::new(super::bridge_store::Store::default())).await;
    let result = invokes::send(
        &f.router,
        Method::POST,
        &format!("/api/v1/bridges/{}/invoke-grants", f.id),
        "owner-a",
        Some("fixture-consent"),
        Some(invokes::issue_body()),
    )
    .await;
    assert_eq!(result.0, axum::http::StatusCode::CREATED);
    let grant = result.1["result"]["id"].as_str().unwrap().to_owned();
    let management = f
        .definitions
        .management_lease(&context("owner-a"), None, &BridgeId(f.id.clone()), 1)
        .await
        .unwrap();
    let proof = f.service.prepare_enable(&management).await.unwrap();
    f.definitions
        .mutate_with_enable_proof(
            &context("owner-a"),
            None,
            BridgeMutation {
                id: BridgeId(f.id.clone()),
                expected_revision: 1,
                idempotency_key: "enable-source-controls-fixture".into(),
                change: BridgeChange::Enable,
            },
            proof,
        )
        .await
        .unwrap();
    let service = Arc::new(BridgeSourceControlService::new(
        (*f.service).clone(),
        Arc::new(Memory::default()),
    ));
    Fixture {
        router: f
            .router
            .clone()
            .merge(super::super::bridge_source_control_router(Some(service))),
        path: format!("/api/v1/bridge-invoke-grants/{grant}/source-controls"),
        grant,
        bridge: f,
    }
}
pub(in crate::rest) fn body(revision: u64, action: &str) -> Value {
    json!({"expected_bridge_revision":2,"expected_grant_revision":1,"expected_policy_revision":revision,"action":action})
}
