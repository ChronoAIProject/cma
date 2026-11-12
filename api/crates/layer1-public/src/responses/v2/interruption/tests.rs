use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{
    AgentRuntimeInput as ConversationInput, AgentRuntimePromptResponse as PromptResponse,
    AgentRuntimeTurnStopReason as TurnStopReason, AgentRuntimeUpdate as ConversationUpdate,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::ResourcePolicyFence,
    sandbox::{
        agent_io::*,
        store::{AgentLogStore, AgentStore, PageRequest, SandboxMutationGuard, SandboxStore},
        AgentLogKind, AgentMessageAssembly, AgentOperationService, AgentRuntimeConnection,
        AgentRuntimeError, AgentRuntimeRegistry, AgentRuntimeTarget, AgentUpdateRecorder,
        DurableAgentUpdateRecorder,
    },
    AgentId, RuntimeCapabilities, RuntimeCapability,
};
use serde_json::{json, Value};

use super::super::controls::fixtures::{call, context, fixture, intent, Fixture, SOURCE};
const PATH: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/interrupt";

struct Runtime(Arc<Mutex<Vec<ConversationInput>>>);
#[async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let writes = self.0.clone();
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Interrupt,
            ]),
            input: layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, move |input| {
                writes.lock().unwrap().push(input);
                async { Ok(()) }
            }),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

async fn body(f: &Fixture) -> Value {
    let mut body = intent(f, json!({"type":"cancel"})).await;
    body.as_object_mut().unwrap().remove("command");
    body["text"] = json!("Start a new turn\n<attachment>kept</attachment>");
    body
}

async fn reserve(f: &Fixture, key: &str) -> AdmittedAgentCommand {
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
    let source = f
        .store
        .get_command(&AgentCommandId(SOURCE.into()))
        .await
        .unwrap()
        .unwrap();
    let mut envelope = source.receipt.envelope.clone();
    envelope.command_id = AgentCommandId::generate();
    envelope.command = AgentCommand::message(key);
    envelope.request_fingerprint = envelope.command.fingerprint().unwrap();
    let CommandReservationResult::Created(command) = f
        .store
        .reserve_command(CommandReservation {
            envelope,
            binding: None,
            nyxid_trace: None,
            key_sha256: command_key_digest(key).unwrap(),
            response_id: Some(AgentResponseId::generate()),
            admitted_at: source.receipt.admitted_at,
            queue_deadline: source.queue_deadline,
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
        .unwrap()
    else {
        panic!("new queued input")
    };
    *command
}

async fn claim(f: &Fixture) -> Option<AdmittedAgentCommand> {
    let now = SystemTime::now();
    f.store
        .claim_next_command(CommandClaimRequest {
            agent_id: AgentId("agt_fixture".into()),
            holder: "interrupt-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
}

async fn end(f: &Fixture, reason: TurnStopReason) {
    DurableAgentUpdateRecorder::new(f.store.clone())
        .record_update(
            &AgentId("agt_fixture".into()),
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(reason)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn interrupt_stops_original_then_starts_distinct_message_before_preserved_fifo() {
    let f = fixture(true).await;
    let first = reserve(&f, "existing-one").await;
    let second = reserve(&f, "existing-two").await;
    let request = body(&f).await;
    let (status, accepted) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("interrupt-one"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let cancel_id = AgentCommandId(
        accepted["control"]["receipt"]["command_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    let new_id = AgentCommandId(
        accepted["response"]["receipt"]["command_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    assert_ne!(
        accepted["response"]["receipt"]["response_id"],
        "resp_control_source"
    );
    let writes = Arc::new(Mutex::new(Vec::new()));
    let operations = AgentOperationService::new(f.store.clone(), Arc::new(Runtime(writes.clone())));
    let cancel = claim(&f).await.unwrap();
    assert_eq!(cancel.receipt.envelope.command_id, cancel_id);
    operations
        .execute_claimed_command(&context("owner-a").caller, &cancel)
        .await
        .unwrap();
    f.store.reconcile_command(&cancel_id).await.unwrap();
    assert!(matches!(
        writes.lock().unwrap().as_slice(),
        [ConversationInput::Cancel]
    ));
    assert!(
        claim(&f).await.is_none(),
        "delivery alone cannot start replacement"
    );
    end(&f, TurnStopReason::Cancelled).await;
    assert_eq!(
        f.store
            .reconcile_command(&AgentCommandId(SOURCE.into()))
            .await
            .unwrap()
            .status,
        AgentResponseStatus::Cancelled
    );
    let replacement = claim(&f).await.unwrap();
    assert_eq!(replacement.receipt.envelope.command_id, new_id);
    operations
        .execute_claimed_command(&context("owner-a").caller, &replacement)
        .await
        .unwrap();
    assert!(
        matches!(writes.lock().unwrap().as_slice(), [ConversationInput::Cancel, ConversationInput::RecoverablePrompt { content, .. }]
        if serde_json::to_value(content).unwrap() == json!([{"type":"text","text":request["text"]}]))
    );
    f.store.reconcile_command(&new_id).await.unwrap();
    let logs = f
        .store
        .agent_log(&AgentId("agt_fixture".into()), 0, 100)
        .await
        .unwrap();
    let turns: Vec<_> = logs
        .iter()
        .filter_map(|entry| match entry.kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => Some(turn_sequence),
            _ => None,
        })
        .collect();
    assert_eq!(turns.len(), 2);
    assert!(turns[1] > turns[0]);
    let (status, recovered) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("interrupt-one"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{recovered}");
    assert_eq!(
        recovered["control"]["receipt"],
        accepted["control"]["receipt"]
    );
    assert_eq!(
        recovered["response"]["receipt"],
        accepted["response"]["receipt"]
    );
    assert_eq!(
        writes.lock().unwrap().len(),
        2,
        "exact retry does not write runtime input"
    );
    for queued in [first, second] {
        end(&f, TurnStopReason::EndTurn).await;
        let next = claim(&f).await.unwrap();
        assert_eq!(
            next.receipt, queued.receipt,
            "ordinary queue retains identity and FIFO order"
        );
        operations
            .execute_claimed_command(&context("owner-a").caller, &next)
            .await
            .unwrap();
        f.store
            .reconcile_command(&next.receipt.envelope.command_id)
            .await
            .unwrap();
    }
    let mut changed = request;
    changed["text"] = json!("different content");
    let (status, conflict) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("interrupt-one"),
        Some(changed),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "agent_command_key_conflict");
}

#[tokio::test]
async fn interruption_capacity_failure_rolls_back_both_commands_and_original_key() {
    let f = fixture(true).await;
    for i in 0..63 {
        reserve(&f, &format!("queued-{i}")).await;
    }
    let request = body(&f).await;
    let before = f
        .store
        .command_work_page(&PageRequest::new(100, None).unwrap())
        .await
        .unwrap()
        .items;
    let (status, rejected) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("full-interrupt"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{rejected}");
    assert_eq!(rejected["error"]["code"], "agent_command_queue_full");
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::new(100, None).unwrap())
            .await
            .unwrap()
            .items,
        before
    );
    assert!(
        claim(&f).await.is_none(),
        "no partial cancel escapes a full-queue transaction"
    );
}

#[tokio::test]
async fn interruption_requires_current_authority_and_strict_nonempty_input() {
    let f = fixture(true).await;
    let request = body(&f).await;
    let (status, _) = call(
        &f.router,
        Method::POST,
        PATH,
        "other-owner",
        Some("key"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for text in [json!(""), json!(" "), Value::Null] {
        let mut invalid = request.clone();
        invalid["text"] = text;
        let (status, rejected) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("key"),
            Some(invalid),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(rejected["error"]["code"], "invalid_interrupt_request");
    }
    let mut invalid = request;
    invalid["actor"] = json!("forged");
    let (status, rejected) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("key"),
        Some(invalid),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(rejected["error"]["code"], "invalid_interrupt_request");
}
