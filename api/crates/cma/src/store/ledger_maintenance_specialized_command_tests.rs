//! Specialized command responses still write resource fences on replay/lookup.
use super::*;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceStore,
};

#[derive(Clone, Copy)]
enum Action {
    Cancel,
    Edit,
}
const ACTIONS: [Action; 2] = [Action::Cancel, Action::Edit];

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Cancel(QueuedCancellationOutcome),
    Edit(QueuedInputOutcome),
}

async fn change(
    store: &MongoSandboxStore,
    request: &QueuedCancellationReservation,
    action: Action,
) -> Result<Outcome, CommandLedgerError> {
    match action {
        Action::Cancel => store
            .cancel_queued_response(request.clone())
            .await
            .map(Outcome::Cancel),
        Action::Edit => store
            .edit_queued_input(QueuedInputReservation {
                cancellation: request.clone(),
                text: "Revised retained fixture input.".into(),
            })
            .await
            .map(Outcome::Edit),
    }
}

async fn queued(
    store: &MongoSandboxStore,
) -> (LedgerMaintenanceAdmission, QueuedCancellationReservation) {
    let input = support::fixture(store, true).await;
    let source = &input.accepted.plan().source;
    let agent = &source.agents[0];
    let response = match store
        .reserve_command(reservation(agent, &source.sandbox, "specialized-source"))
        .await
        .unwrap()
    {
        CommandReservationResult::Created(value) => value,
        _ => panic!("expected new source command"),
    };
    let mut command = reservation(agent, &source.sandbox, "specialized-control");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    let request = QueuedCancellationReservation {
        command,
        target: QueuedCancellationTarget {
            response_id: response.receipt.response_id.clone().unwrap(),
            source_command_id: response.receipt.envelope.command_id.clone(),
            expected_command_revision: response.revision,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
    };
    request.validate().unwrap();
    (input, request)
}

async fn hold(store: &MongoSandboxStore, mut input: LedgerMaintenanceAdmission, v1: bool) {
    if v1 {
        let mut plan = input.accepted.plan().clone();
        plan.source = store
            .inspect_ledger_maintenance(&input.caller, &plan.source.sandbox.id)
            .await
            .unwrap()
            .source;
        input.accepted = support::artifact(&plan);
        store
            .admit_ledger_maintenance(LedgerMaintenanceAdmissionPolicy { enabled: true }, input)
            .await
            .unwrap();
    } else {
        let (policy, next) =
            support::pre_exclusion(store, &input.accepted.plan().source.sandbox, NOW).await;
        store.admit_pre_exclusion(policy, next).await.unwrap();
    }
}

fn assert_applied(outcome: &Outcome, action: Action) {
    let (record, response) = match outcome {
        Outcome::Cancel(QueuedCancellationOutcome::Applied(value)) => {
            (&value.record, &value.response)
        }
        Outcome::Edit(QueuedInputOutcome::Applied(value)) => (&value.record, &value.response),
        _ => panic!("expected newly applied command effect"),
    };
    assert_eq!(record.status, AgentResponseStatus::Completed);
    assert_eq!(
        response.status,
        match action {
            Action::Cancel => AgentResponseStatus::Cancelled,
            Action::Edit => AgentResponseStatus::Queued,
        }
    );
    assert!(response.work.lease.is_none());
    assert!(response.work.dispatch.is_none());
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and specialized command hooks"]
async fn queued_changes_have_positive_controls_and_both_holds_refuse_all_writes() {
    for action in ACTIONS {
        for held in [0, 1, 2] {
            let (client, name, store) = database().await;
            let (input, request) = queued(&store).await;
            if held != 0 {
                hold(&store, input, held == 1).await;
            }
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let before = all_rows(&store).await;
            let result = change(&reopened, &request, action).await;
            if held == 0 {
                assert_applied(&result.unwrap(), action);
                assert_ne!(all_rows(&store).await, before);
            } else {
                assert_eq!(result, Err(CommandLedgerError::NotAdmissible));
                assert_eq!(all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and specialized command hooks"]
async fn queued_replay_preserves_original_result_but_refuses_new_fences_under_hold() {
    for action in ACTIONS {
        for v1 in [false, true] {
            let (client, name, store) = database().await;
            let (input, request) = queued(&store).await;
            let original = change(&store, &request, action).await.unwrap();
            assert_applied(&original, action);
            assert_eq!(change(&store, &request, action).await.unwrap(), original);
            hold(&store, input, v1).await;
            let before = all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            assert_eq!(
                change(&reopened, &request, action).await,
                Err(CommandLedgerError::NotAdmissible)
            );
            assert_eq!(all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and specialized command hooks"]
async fn stale_queued_changes_cannot_commit_not_admitted_fences_under_hold() {
    for action in ACTIONS {
        for v1 in [false, true] {
            let (client, name, store) = database().await;
            let (input, request) = queued(&store).await;
            change(&store, &request, Action::Cancel).await.unwrap();
            let mut stale = request;
            stale.command.key_sha256 = command_key_digest("stale-specialized-control").unwrap();
            stale.command.envelope.command_id = layer2_core::ids::AgentCommandId::generate();
            let expected = match action {
                Action::Cancel => Outcome::Cancel(QueuedCancellationOutcome::NotAdmitted),
                Action::Edit => Outcome::Edit(QueuedInputOutcome::NotAdmitted),
            };
            assert_eq!(change(&store, &stale, action).await.unwrap(), expected);
            hold(&store, input, v1).await;
            let before = all_rows(&store).await;
            assert_eq!(
                change(&store, &stale, action).await,
                Err(CommandLedgerError::NotAdmissible)
            );
            assert_eq!(all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and specialized command hooks"]
async fn targeted_missing_lookup_and_inspection_are_writers_and_refuse_both_holds() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (input, queued) = queued(&store).await;
        let request = TargetedControlReservation {
            command: queued.command,
            target: TargetedResponse {
                response_id: queued.target.response_id,
                turn_sequence: 1,
            },
            observed_agent_revision: queued.observed_agent_revision,
            agent_created_at: queued.agent_created_at,
            retry_deadline: None,
        };
        let before = all_rows(&store).await;
        assert_eq!(
            store
                .lookup_targeted_control(request.clone())
                .await
                .unwrap(),
            None
        );
        assert_ne!(all_rows(&store).await, before);
        let before = all_rows(&store).await;
        assert_eq!(
            store
                .inspect_targeted_control(request.clone())
                .await
                .unwrap(),
            TargetedControlInspection::Missing
        );
        assert_ne!(all_rows(&store).await, before);
        hold(&store, input, v1).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let before = all_rows(&store).await;
        assert_eq!(
            reopened.lookup_targeted_control(request.clone()).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(
            reopened.inspect_targeted_control(request).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and specialized command hooks"]
async fn queued_mutations_fail_closed_on_missing_hold_audits() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (input, request) = queued(&store).await;
        hold(&store, input, v1).await;
        let audits = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            transaction::audit_collection(&store)
        };
        audits.delete_many(doc! {}).await.unwrap();
        let before = all_rows(&store).await;
        for action in ACTIONS {
            assert_eq!(
                change(&store, &request, action).await,
                Err(CommandLedgerError::Corrupt)
            );
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and specialized command hooks"]
async fn failed_specialized_receipt_insert_rolls_back_source_lane_and_parent_fences() {
    for action in ACTIONS {
        let (client, name, store) = database().await;
        let (_, request) = queued(&store).await;
        // The existing source may be updated; only the new terminal receipt is
        // rejected, after the source/lane/parent effects have been attempted.
        client
            .database(&name)
            .run_command(doc! {
                "collMod":store.agent_commands.name(),
                "validator":{"$or":[
                    {"_id":request.target.source_command_id.as_str()},
                    {"required_fixture_field":{"$exists":true}},
                ]},
                "validationLevel":"strict", "validationAction":"error",
            })
            .await
            .unwrap();
        let before = all_rows(&store).await;
        assert_eq!(
            change(&store, &request, action).await,
            Err(CommandLedgerError::Unavailable)
        );
        assert_eq!(all_rows(&store).await, before);
        client
            .database(&name)
            .run_command(doc! {"collMod":store.agent_commands.name(),"validator":{}})
            .await
            .unwrap();
        assert_applied(&change(&store, &request, action).await.unwrap(), action);
        client.database(&name).drop().await.unwrap();
    }
}
