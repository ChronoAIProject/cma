//! Historical evidence only: these records grant no execution authority.
use super::{fixture, get};
use axum::http::StatusCode;
use layer2_core::{
    bridge::{partition::PartitionSourcePin, status::*},
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{agent_io::*, store::AgentStore, AgentOperationDelivery, AgentStatus},
};
use serde_json::json;

fn command(
    f: &fixture::Fixture,
    order: u64,
    status: AgentResponseStatus,
    hold: ChannelCommandHold,
    turn: Option<u64>,
) -> PartitionCommandEvidence {
    let evidence = f.memory.evidence.lock().unwrap();
    let allocation = &evidence.allocation;
    let mut source = evidence.first_source.clone().unwrap();
    source.id = format!("{order:064x}");
    source.message_id = format!("message-{order}");
    let mut observation = serde_json::to_value(source.capabilities.take().unwrap()).unwrap();
    observation["source"] =
        serde_json::to_value(PartitionSourcePin::for_inbox(&source).unwrap()).unwrap();
    observation["message_id"] = json!(source.message_id);
    source.capabilities = Some(serde_json::from_value(observation).unwrap());
    let mut message = AgentCommand::message("private fixture input")
        .canonical()
        .unwrap();
    let AgentCommand::Message { metadata, .. } = &mut message else {
        unreachable!()
    };
    for (key, value) in [
        ("channel_inbox_id", source.id.clone()),
        (
            "channel_body_sha256",
            source.attestation.body_sha256.clone(),
        ),
        ("channel_message_id", source.message_id.clone()),
        (
            "channel_config_revision",
            source.pin.config_revision.to_string(),
        ),
        ("channel_allocation_id", allocation.id.clone()),
    ] {
        metadata.insert(key.into(), value);
    }
    let command_id = AgentCommandId(format!("cmd_{order}"));
    let response_id = AgentResponseId(format!("resp_{order}"));
    let agent_id = allocation.placement.target().agent_id.clone();
    let envelope = AgentCommandEnvelope {
        schema_version: AGENT_IO_SCHEMA_VERSION,
        command_id: command_id.clone(),
        agent_id: agent_id.clone(),
        owner: allocation.owner.clone(),
        actor: AgentActor::Channel {
            bridge_id: f.bridge.as_str().into(),
            grant_id: "historical-grant".into(),
            platform: source.envelope.platform.clone(),
            chat_id: source.envelope.chat_id.clone(),
            sender_id: source.envelope.sender_id.clone(),
        },
        source: AgentCommandSource {
            kind: AgentSourceKind::Channel,
            reference: source.id.clone(),
        },
        correlation_id: source.id.clone(),
        request_fingerprint: message.fingerprint().unwrap(),
        authorization: AgentAuthorizationRevision {
            authority_id: "historical-channel".into(),
            policy_revision: 1,
            grant_revision: Some(1),
        },
        command: message,
    };
    let sidecar = ChannelCommandSidecar {
        schema_version: 1,
        inbox_id: source.id.clone(),
        allocation_id: allocation.id.clone(),
        body_sha256: source.attestation.body_sha256.clone(),
        command_id: command_id.clone(),
        response_id: response_id.clone(),
        request_fingerprint: envelope.request_fingerprint.clone(),
        execution_hold: hold,
    };
    let command = AdmittedAgentCommand {
        receipt: AdmissionReceipt {
            envelope,
            response_id: Some(response_id),
            admitted_at: "2026-09-15T06:00:00Z".into(),
            agent_order: order,
            event_cursor: 0,
        },
        key_sha256: "a".repeat(64),
        queue_deadline: "2026-09-15T07:00:00Z".into(),
        status,
        revision: 1,
        target_turn: None,
        work: CommandWorkState {
            dispatch: turn.map(|turn_sequence| CommandDispatch {
                claim: CommandClaim {
                    agent_id,
                    command_id,
                    holder: "historical-worker".into(),
                    fence: 1,
                },
                operation_sequence: 1,
                turn_sequence,
                runtime_generation: 1,
                fenced_at: "2026-09-15T06:01:00Z".into(),
                delivery: AgentOperationDelivery::ExecutionObserved,
            }),
            ..Default::default()
        },
    };
    command.validate().unwrap();
    PartitionCommandEvidence {
        command,
        sidecar,
        source: Some(source),
    }
}

#[tokio::test]
async fn partition_pending_correlates_queue_worker_approval_and_latest_response() {
    let f = fixture::fixture(false).await;
    let id = f
        .memory
        .evidence
        .lock()
        .unwrap()
        .allocation
        .placement
        .target()
        .agent_id
        .clone();
    let mut agent = f.resources.get_agent(&id).await.unwrap().unwrap();
    let expected = agent.revision;
    agent.status = AgentStatus::WaitingApproval;
    agent.conversation.active_turn = Some(7);
    agent.revision += 1;
    f.resources.save_agent(expected, agent).await.unwrap();
    let pending = vec![
        command(
            &f,
            1,
            AgentResponseStatus::Queued,
            ChannelCommandHold::Released,
            None,
        ),
        command(
            &f,
            2,
            AgentResponseStatus::WaitingAction,
            ChannelCommandHold::WorkerUnavailable,
            None,
        ),
        command(
            &f,
            3,
            AgentResponseStatus::WaitingAction,
            ChannelCommandHold::Released,
            Some(7),
        ),
    ];
    *f.memory.activity.lock().unwrap() = PartitionActivityEvidence {
        pending: pending.clone(),
        latest: pending.last().cloned(),
        observed_at_ms: 1000,
    };
    let (status, body) = get(&f, "owner-a", &format!("/{}", f.allocation)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (index, wait) in ["none", "worker", "approval"].iter().enumerate() {
        assert_eq!(body["activity"]["pending"][index]["waiting_for"], *wait);
    }
    assert_eq!(body["activity"]["latest"]["response_id"], "resp_3");
    let terminal = command(
        &f,
        4,
        AgentResponseStatus::Completed,
        ChannelCommandHold::Released,
        Some(8),
    );
    f.memory.activity.lock().unwrap().latest = Some(terminal);
    assert_eq!(
        get(&f, "owner-a", &format!("/{}", f.allocation)).await.1["activity"]["latest"]
            ["waiting_for"],
        "none"
    );
}

#[tokio::test]
async fn partition_pending_is_bounded_and_missing_historical_sources_are_explicit() {
    let f = fixture::fixture(false).await;
    let pending: Vec<_> = (1..=65)
        .map(|order| {
            command(
                &f,
                order,
                AgentResponseStatus::WaitingAction,
                ChannelCommandHold::WorkerUnavailable,
                None,
            )
        })
        .collect();
    *f.memory.activity.lock().unwrap() = PartitionActivityEvidence {
        pending: pending.clone(),
        latest: pending.last().cloned(),
        observed_at_ms: 1000,
    };
    let (status, body) = get(&f, "owner-a", &format!("/{}", f.allocation)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activity"]["pending"].as_array().unwrap().len(), 64);
    assert_eq!(body["activity"]["pending_truncated"], true);
    {
        let mut activity = f.memory.activity.lock().unwrap();
        activity.pending[0].source = None;
    }
    let body = get(&f, "owner-a", &format!("/{}", f.allocation)).await.1;
    assert_eq!(body["activity"]["source_unavailable"], true);
    assert_eq!(body["activity"]["pending"].as_array().unwrap().len(), 63);
}

#[tokio::test]
async fn partition_rejects_cross_allocation_command_even_with_valid_envelope_integrity() {
    let f = fixture::fixture(false).await;
    let mut wrong = command(
        &f,
        1,
        AgentResponseStatus::Queued,
        ChannelCommandHold::Released,
        None,
    );
    let envelope = &mut wrong.command.receipt.envelope;
    let AgentCommand::Message { metadata, .. } = &mut envelope.command else {
        unreachable!()
    };
    metadata.insert("channel_allocation_id".into(), "another".into());
    envelope.request_fingerprint = envelope.command.fingerprint().unwrap();
    wrong.sidecar.request_fingerprint = envelope.request_fingerprint.clone();
    wrong.command.validate().unwrap();
    *f.memory.activity.lock().unwrap() = PartitionActivityEvidence {
        pending: vec![wrong.clone()],
        latest: Some(wrong),
        observed_at_ms: 1000,
    };
    assert_eq!(
        get(&f, "owner-a", &format!("/{}", f.allocation)).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
