use super::{cursor, error::Error, fixtures::*, router, AgentV2State};
use async_trait::async_trait;
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
    Router,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::*,
    sandbox::{agent_io::*, store::*, *},
    AgentId, SubjectId,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use tower::ServiceExt;

mod counters;
mod parity;
mod races;

const HEAD: &str = "/api/v2/agents/agt_fixture/events/head";

async fn raw(app: &Router, path: &str, subject: &str, prefer: &str) -> (StatusCode, Vec<u8>) {
    let mut request = request(Method::GET, path, subject, None, None);
    if !prefer.is_empty() {
        request
            .headers_mut()
            .insert("prefer", prefer.parse().unwrap());
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}

struct GateCommands {
    inner: Arc<dyn AgentCommandStore>,
    entered: Arc<Notify>,
    resume: Arc<Notify>,
    fail: bool,
}
#[async_trait]
impl AgentCommandStore for GateCommands {
    async fn reserve_command(
        &self,
        _: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        panic!("history must not admit commands")
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
        self.inner.get_operation_command(id, operation).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.entered.notify_one();
        self.resume.notified().await;
        if self.fail {
            Err(CommandLedgerError::Unavailable)
        } else {
            self.inner.get_command(id).await
        }
    }
}

fn gated(f: &Fixture, fail: bool) -> (AgentV2State, Arc<Notify>, Arc<Notify>) {
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let commands = AgentCommandService::new(
        Arc::new(GateCommands {
            inner: f.store.clone(),
            entered: entered.clone(),
            resume: resume.clone(),
            fail,
        }),
        f.access.clone(),
        Arc::new(NoAdmission),
    );
    (
        AgentV2State::new(f.state.reads.clone(), commands).with_single_fence_pages(true),
        entered,
        resume,
    )
}

async fn entered(gate: &Notify) {
    tokio::time::timeout(Duration::from_secs(15), gate.notified())
        .await
        .unwrap();
}

async fn revoke(f: &Fixture, share: ResourceShare) {
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "buffered-revoke".into(),
            },
        )
        .await
        .unwrap();
}
