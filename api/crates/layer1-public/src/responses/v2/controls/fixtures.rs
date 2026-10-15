use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request},
    Router,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::ResourcePolicyFence,
    sandbox::{
        agent_io::*,
        store::{AgentStore, SandboxMutationGuard, SandboxStore},
        AgentOperationService, AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeRegistry,
        AgentRuntimeTarget,
    },
    AgentId, RuntimeCapabilities, RuntimeCapability,
};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

pub(crate) use super::super::tests::{context, Fixture};
pub(crate) const PATH: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/controls";
pub(crate) const SOURCE: &str = "cmd_control_source";

struct Runtime;
#[async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Interrupt,
                RuntimeCapability::Steer,
            ]),
            input: layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, |_| async { Ok(()) }),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

/// Real service/store/router composition, with fixed retained source identity.
pub(crate) async fn fixture(bound: bool) -> Fixture {
    let f = super::super::tests::fixture().await;
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let parent = f
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let command = AgentCommand::message("Private source input");
    f.store
        .reserve_command(CommandReservation {
            binding: None,
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId(SOURCE.into()),
                agent_id: agent.id.clone(),
                owner: AgentOwner::Person {
                    subject: agent.owner.clone(),
                },
                actor: context("owner-a").actor,
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "private-source-reference".into(),
                },
                correlation_id: "private-correlation".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                command,
                authorization: AgentAuthorizationRevision {
                    authority_id: "fixture-custody".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
            },
            key_sha256: command_key_digest("source-key").unwrap(),
            response_id: Some(AgentResponseId("resp_control_source".into())),
            admitted_at: "2026-09-05T00:00:00Z".into(),
            queue_deadline: "2030-09-05T00:00:00Z".into(),
            queue_capacity: 64,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&parent),
            policy: ResourcePolicyFence {
                sandbox_id: parent.id,
                sandbox_created_at: parent.created_at,
                policy_revision: 0,
            },
            authority_deadline: None,
            admission_audit: None,
        })
        .await
        .unwrap();
    if bound {
        let now = SystemTime::now();
        let claimed = f
            .store
            .claim_next_command(CommandClaimRequest {
                agent_id: agent.id,
                holder: "controls-fixture-worker".into(),
                now: layer2_core::rfc3339_at(now),
                expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
        AgentOperationService::new(f.store.clone(), Arc::new(Runtime))
            .execute_claimed_command(&context("owner-a").caller, &claimed)
            .await
            .unwrap();
        f.store
            .release_command_claim(
                &claimed.work.lease.as_ref().unwrap().claim,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    f
}

pub(crate) async fn execute_next(f: &Fixture) -> AgentCommandId {
    let now = SystemTime::now();
    let claimed = f
        .store
        .claim_next_command(CommandClaimRequest {
            agent_id: AgentId("agt_fixture".into()),
            holder: "control-delivery".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    AgentOperationService::new(f.store.clone(), Arc::new(Runtime))
        .execute_claimed_command(&context("owner-a").caller, &claimed)
        .await
        .unwrap();
    f.store
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    claimed.receipt.envelope.command_id
}

pub(crate) fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = crate::rest::agents_tests::request(method, path, subject, key, body);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("fixture-credential".into()));
    request
}

pub(crate) async fn call(
    router: &Router,
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (axum::http::StatusCode, Value) {
    crate::rest::agents_tests::json_response(router, request(method, path, subject, key, body))
        .await
}

pub(crate) async fn intent(f: &Fixture, command: Value) -> Value {
    let (_, observed) = call(&f.router, Method::GET, PATH, "owner-a", None, None).await;
    json!({"turn_sequence":observed["target"]["turn_sequence"],
        "expected_agent_revision":observed["agent_revision"], "command":command})
}

pub(crate) async fn pending_approval(f: &Fixture, approval_id: &str) {
    use layer2_core::sandbox::{
        store::{AgentConversationMutation, AgentConversationStore},
        AgentLogKind, AgentStatus,
    };
    let current = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let at = layer2_core::now_rfc3339();
    let mut agent = current.clone();
    agent
        .transition(AgentStatus::WaitingApproval, at.clone(), None)
        .unwrap();
    agent.conversation.last_seq += 2;
    f.store
        .commit_agent_conversation(AgentConversationMutation {
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: 1,
            agent,
            appended_at: at,
            agent_logs: vec![
                AgentLogKind::ApprovalRequested {
                    approval_id: approval_id.into(),
                    action: Some("Private action".into()),
                    detail: None,
                    runtime_locator: Some("private-runtime-locator".into()),
                },
                AgentLogKind::StatusChanged {
                    from: current.status,
                    to: AgentStatus::WaitingApproval,
                },
            ],
        })
        .await
        .unwrap();
}
