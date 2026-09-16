use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Writer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn dispatch_observation_keeps_ambiguity_separate_from_waiting_for_a_person() {
    use AgentOperationDelivery as Delivery;
    use AgentResponseStatus as Status;
    assert_eq!(command_outcome(Status::Queued, None), "queued");
    for delivery in [Delivery::DispatchFenced, Delivery::RecoveryRequired] {
        assert_eq!(
            command_outcome(Status::InProgress, Some(delivery)),
            "dispatch_ambiguous"
        );
    }
    for delivery in [Delivery::WriteConfirmed, Delivery::ExecutionObserved] {
        assert_eq!(
            command_outcome(Status::InProgress, Some(delivery)),
            "active"
        );
        assert_eq!(
            command_outcome(Status::WaitingAction, Some(delivery)),
            "waiting_action"
        );
    }
    assert_eq!(
        command_outcome(Status::InProgress, Some(Delivery::RuntimeLost)),
        "runtime_lost"
    );
}

#[test]
fn durable_age_survives_restarts_and_clock_skew_does_not_wrap() {
    let admitted = timestamp_ms("2026-09-16T00:00:00Z").unwrap();
    let restarted = timestamp_ms("2026-09-16T00:06:00Z").unwrap();
    assert_eq!(age_ms(restarted, admitted), 360_000);
    assert!(age_ms(restarted, admitted) >= PENDING_WARN_MS);
    assert_eq!(age_ms(admitted, restarted), 0);
    assert_eq!(age_ms(i64::MAX, i64::MIN), i64::MAX);
    assert_eq!(timestamp_ms("private-not-a-timestamp"), None);
}

#[test]
fn emitted_text_supports_alert_filters_without_exporting_work_identity() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = Writer(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        pending(PendingStage::ChildCleanup, "private-child-route", 1, true);
        mint_unknown("private-mint-target");
        authority_failure(AuthorityFailure::MembershipDenied);
        authority_failure(AuthorityFailure::HumanTokenExpired);
        retirement("private-sandbox", 5, "retention_held");
        credential_expiry("private-expiring-allocation", 1);
    });
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    for expected in [
        "cma::bridge::operations",
        "channel_health",
        "operation=\"pending\"",
        "worker=\"child_cleanup\"",
        "held=true",
        "overdue=true",
        "operation=\"allocation_effect\"",
        "outcome=\"mint_unknown\"",
        "reason=\"membership_denied\"",
        "reason=\"human_token_expired\"",
        "operation=\"retirement\"",
        "stalled=true",
        "work_id_sha256=",
        "operation=\"credential_expiry\"",
        "expired=true",
        "expiring=true",
    ] {
        assert!(
            logs.contains(expected),
            "missing actual text field {expected}: {logs}"
        );
    }
    for private in [
        "private-child-route",
        "private-mint-target",
        "private-sandbox",
        "private-expiring-allocation",
    ] {
        assert!(
            !logs.contains(private),
            "work identity leaked into operational fields"
        );
        assert!(logs.contains(&format!("{:x}", Sha256::digest(private.as_bytes()))));
    }
}

#[test]
fn current_dispatch_observation_wins_and_private_command_fields_are_not_logged() {
    use layer2_core::resource_access::ResourceOwner;
    use layer2_core::sandbox::agent_io::*;
    let envelope = AgentCommandEnvelope {
        schema_version: 1,
        command_id: layer2_core::ids::AgentCommandId("cmd_health".into()),
        agent_id: layer2_core::AgentId("private-agent".into()),
        owner: ResourceOwner::Person {
            subject: layer2_core::SubjectId("private-owner".into()),
        },
        actor: AgentActor::Human {
            subject: layer2_core::SubjectId("private-human".into()),
        },
        source: AgentCommandSource {
            kind: AgentSourceKind::Channel,
            reference: "private-source".into(),
        },
        correlation_id: "private-correlation".into(),
        request_fingerprint: "private-fingerprint".into(),
        authorization: AgentAuthorizationRevision {
            authority_id: "private-authority".into(),
            policy_revision: 1,
            grant_revision: None,
        },
        command: AgentCommand::message("private-prompt"),
    };
    let mut record = AdmittedAgentCommand {
        receipt: AdmissionReceipt {
            envelope,
            response_id: None,
            admitted_at: "2020-01-01T00:00:00Z".into(),
            agent_order: 1,
            event_cursor: 0,
        },
        key_sha256: "private-key".into(),
        queue_deadline: "2020-01-01T00:05:00Z".into(),
        status: AgentResponseStatus::Queued,
        revision: 1,
        target_turn: None,
        work: CommandWorkState::default(),
    };
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = Writer(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        command(&record);
        record.status = AgentResponseStatus::InProgress;
        record.work.dispatch = Some(CommandDispatch {
            claim: CommandClaim {
                agent_id: record.receipt.envelope.agent_id.clone(),
                command_id: record.receipt.envelope.command_id.clone(),
                holder: "private-holder".into(),
                fence: 1,
            },
            operation_sequence: 1,
            turn_sequence: 1,
            runtime_generation: 1,
            fenced_at: "2020-01-01T00:00:00Z".into(),
            delivery: AgentOperationDelivery::DispatchFenced,
        });
        command(&record);
        record.work.observation = Some(CommandObservation {
            event_cursor: 1,
            delivery: AgentOperationDelivery::ExecutionObserved,
        });
        command(&record);
    });
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let lines: Vec<_> = logs.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("queue_overdue=true"));
    assert!(lines[1].contains("dispatch_overdue=true"));
    assert!(lines[2].contains("outcome=\"active\""));
    assert!(lines[2].contains("dispatch_overdue=false"));
    assert!(logs.contains("traffic=\"channel\""));
    assert!(logs.contains("command_id=\"cmd_health\""));
    assert!(
        !logs.contains("private-"),
        "command data leaked into health logs"
    );
}
