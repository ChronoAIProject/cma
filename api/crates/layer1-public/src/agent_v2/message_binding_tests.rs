use super::{fixtures::*, AgentV2State};
use crate::responses::v2::controls::fixtures as controls;
use async_trait::async_trait;
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
    Router,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::agent_io::*,
    AgentId, SubjectId,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

pub(super) async fn steered() -> (Fixture, String) {
    let f = fixture(true, false).await;
    let control_fixture = controls::Fixture {
        router: f.router.clone(),
        store: f.store.clone(),
        access: f.access.clone(),
    };
    let body = controls::intent(
        &control_fixture,
        json!({"type":"steer", "text":"Private source input"}),
    )
    .await;
    let (status, accepted) = call(
        &f.router,
        Method::POST,
        controls::PATH,
        "owner-a",
        Some("message-correlation"),
        Some(body),
    )
    .await;
    assert!(status.is_success(), "{accepted}");
    let command = controls::execute_next(&control_fixture).await.to_string();
    assert_eq!(accepted["control"]["receipt"]["command_id"], command);
    (f, command)
}
async fn preferred(router: &Router, path: &str) -> (StatusCode, String) {
    let mut request = request(Method::GET, path, "owner-a", None, None);
    request
        .headers_mut()
        .insert("prefer", "cma-message-bindings".parse().unwrap());
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn exact_steering_binding_is_opt_in_and_survives_page_and_tail_boundaries() {
    let (f, command) = steered().await;
    let (status, legacy) = call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!legacy.to_string().contains("message_command_id"));
    let (status, raw) = preferred(&f.router, SNAPSHOT).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let page: Value = serde_json::from_str(&raw).unwrap();
    let bound: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item.get("message_command_id").is_some())
        .collect();
    assert_eq!(bound.len(), 1);
    let item = bound[0];
    assert_eq!(item["message_command_id"], command);
    assert_eq!(item["kind"]["role"], "user");
    assert_eq!(item["kind"]["content"], "Private source input");
    assert!(item["response_binding"].is_null());
    let sequence = item["sequence"].as_u64().unwrap();
    let incarnation = page["incarnation"].as_str().unwrap();
    let through = legacy["items"].as_array().unwrap().last().unwrap()["sequence"]
        .as_u64()
        .unwrap();
    let path = format!(
        "{SNAPSHOT}?limit=1&page_cursor={}",
        super::cursor::page(incarnation, through, sequence - 1)
    );
    let (status, raw) = preferred(&f.router, &path).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let single: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(single["items"][0]["message_command_id"], command);
    let path = format!(
        "{EVENTS}?follow=false&after_cursor={}",
        super::cursor::event(incarnation, sequence - 1)
    );
    let (status, raw) = preferred(&f.router, &path).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let event: Value = serde_json::from_str(
        raw.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event["message_command_id"], command);
}

struct Altered {
    inner: Arc<dyn AgentCommandStore>,
    fault: &'static str,
}
#[async_trait]
impl AgentCommandStore for Altered {
    async fn reserve_command(
        &self,
        _: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        panic!("read must not admit commands")
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
    async fn get_operation_command(
        &self,
        id: &AgentId,
        operation: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        if self.fault == "unavailable" {
            return Err(CommandLedgerError::Unavailable);
        }
        let mut record = self
            .inner
            .get_operation_command(id, operation)
            .await?
            .unwrap();
        let dispatch = record.work.dispatch.as_mut().unwrap();
        match self.fault {
            "owner" => {
                record.receipt.envelope.owner = AgentOwner::Person {
                    subject: SubjectId::new("foreign-owner"),
                }
            }
            "agent" => {
                record.receipt.envelope.agent_id = AgentId("foreign-agent".into());
                dispatch.claim.agent_id = record.receipt.envelope.agent_id.clone();
            }
            "operation" => dispatch.operation_sequence += 1,
            "fence" => dispatch.fenced_at = "2026-09-05T00:00:00Z".into(),
            "incarnation" => record.receipt.admitted_at = "2020-09-05T00:00:00Z".into(),
            "kind" => {
                record.receipt.envelope.command = AgentCommand::Interrupt;
                record.receipt.envelope.request_fingerprint =
                    record.receipt.envelope.command.fingerprint().unwrap();
            }
            _ => unreachable!(),
        }
        Ok(Some(record))
    }
}
#[tokio::test]
async fn steering_correlation_fails_closed_on_wrong_ledger_evidence() {
    for enabled in [false, true] {
        for fault in [
            "owner",
            "agent",
            "operation",
            "fence",
            "incarnation",
            "kind",
            "unavailable",
        ] {
            let (f, _) = steered().await;
            let commands = AgentCommandService::new(
                Arc::new(Altered {
                    inner: f.store.clone(),
                    fault,
                }),
                f.access.clone(),
                Arc::new(NoAdmission),
            );
            let router = super::router(
                AgentV2State::new(f.state.reads.clone(), commands).with_single_fence_pages(enabled),
            );
            let (status, raw) = preferred(&router, SNAPSHOT).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{fault}: {raw}");
            assert!(!raw.contains("foreign-owner"));
            assert!(!raw.contains("foreign-agent"));
            let (status, _) = call(&router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "unrequested extension affected old readers: {fault}"
            );
        }
    }
}
