//! Operator control is shared across stores, independent of spend metering.
use super::*;
use layer2_core::admission::{AdmissionCause, AdmissionControlStore};
use layer2_core::budget::PlatformSpendLedger;

async fn stop(client: &Client, name: &str) {
    crate::store::admission_control::MongoAdmissionControlStore::open(client, name)
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain new admission",
            true,
            100,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_second_store_refuses_every_fresh_resource_and_message_with_metering_off() {
    let (client, name) = database("operator_fresh").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let ready = ready_workspace(&parent, "wks_operator_ready", "project");
    store.create_workspace(ready.clone()).await.unwrap();
    let accepted = request(&agent, &parent, "accepted");
    let receipt = store.reserve_command(accepted.clone()).await.unwrap();
    let second = MongoSandboxStore::open(&client, &name).await.unwrap();
    stop(&client, &name).await;
    assert!(!second.admission_state(100).await.unwrap().configured);
    assert!(second
        .admission_state(100)
        .await
        .unwrap()
        .closed_for_registration());
    let paused = SandboxStoreError::AdmissionPaused;
    assert_eq!(
        second
            .admit_sandbox(admission("sbx_operator_fresh", 8))
            .await
            .unwrap_err(),
        paused
    );
    assert_eq!(
        second
            .admit_workspace(workspace_admission(workspace(
                &parent,
                "wks_operator_fresh",
                "new"
            )))
            .await
            .unwrap_err(),
        paused
    );
    assert_eq!(
        second
            .admit_agent(agent_admission(
                &ready,
                "operator_fresh",
                "agt_operator_fresh"
            ))
            .await
            .unwrap_err(),
        paused
    );
    assert_eq!(
        second
            .reserve_command(request(&agent, &parent, "refused"))
            .await
            .unwrap_err(),
        CommandLedgerError::AdmissionPaused
    );
    assert_eq!(
        record(second.reserve_command(accepted).await.unwrap()),
        record(receipt)
    );
    assert_eq!(
        second.model_budget.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(
        second
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn continuation_resource_steps_and_exact_replays_do_not_write_hot_settings() {
    let (client, name) = database("operator_continuation").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = active_sandbox("sbx_operator_children");
    parent.profile.digest = layer2_core::profiles::content_digest(&parent.profile.content).unwrap();
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut ready = ready_workspace(&parent, "wks_operator_ready", "project");
    ready.profile.digest = layer2_core::profiles::content_digest(&ready.profile.content).unwrap();
    store.create_workspace(ready.clone()).await.unwrap();
    stop(&client, &name).await;
    let before = store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .await
        .unwrap();
    let mut sandbox = provisioning_sandbox("sbx_operator_continued");
    sandbox.name = "continued".into();
    sandbox.profile.digest =
        layer2_core::profiles::content_digest(&sandbox.profile.content).unwrap();
    sandbox.origin = layer2_core::sandbox::SandboxOrigin::Trigger {
        launch_id: "launch-before-stop".into(),
    };
    let admission = SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("continued".into()),
        8,
        NOW.into(),
    )
    .unwrap();
    assert_eq!(admission.admission_cause(), AdmissionCause::Continuation);
    let first = store.admit_sandbox(admission.clone()).await.unwrap();
    let SandboxAdmissionResult::Created(created) = first else {
        panic!("new continuation expected")
    };
    assert!(
        matches!(store.admit_sandbox(admission).await.unwrap(), SandboxAdmissionResult::Existing(existing) if existing.id == created.id)
    );
    let mut work = workspace_admission(workspace(&parent, "wks_operator_continued", "continued"));
    work.kind = layer2_core::sandbox::store::WorkspaceAdmissionKind::Trigger;
    work.workspace.origin = layer2_core::sandbox::WorkspaceOrigin::Trigger {
        launch_id: "launch-before-stop".into(),
    };
    work.workspace.profile.digest =
        layer2_core::profiles::content_digest(&work.workspace.profile.content).unwrap();
    assert!(store.admit_workspace(work).await.is_ok());
    let mut child = agent_admission(&ready, "operator_continued", "agt_operator_continued");
    child.origin = layer2_core::sandbox::AgentOrigin::Trigger {
        trigger_id: "trigger".into(),
        launch_id: "launch-before-stop".into(),
    };
    child.profile.digest = layer2_core::profiles::content_digest(&child.profile.content).unwrap();
    assert!(store.admit_agent(child).await.is_ok());
    assert_eq!(
        store
            .budget_settings
            .find_one(doc! {"_id": "platform"})
            .await
            .unwrap(),
        before
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stop_races_admissions_but_every_admission_started_after_acknowledgement_is_refused() {
    let (client, name) = database("operator_race").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ((), results) = tokio::join!(stop(&client, &name), async {
        let mut results = Vec::new();
        for n in 0..8 {
            results.push(
                second
                    .admit_sandbox(admission(&format!("sbx_race_{n}"), 16))
                    .await,
            );
        }
        results
    });
    assert!(results
        .iter()
        .all(|result| result.is_ok() || matches!(result, Err(SandboxStoreError::AdmissionPaused))));
    for n in 0..8 {
        assert_eq!(
            first
                .admit_sandbox(admission(&format!("sbx_after_{n}"), 16))
                .await
                .unwrap_err(),
            SandboxStoreError::AdmissionPaused
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_budgeted_message_keeps_spend_precedence_and_no_second_reservation() {
    let (client, name) = database("operator_budgeted").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    super::budget_tests::configure_metered(&store, &agent.owner).await;
    stop(&client, &name).await;
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "paused-budgeted"))
            .await
            .unwrap_err(),
        CommandLedgerError::AdmissionPaused
    );
    let start = super::budget_tests::open_platform()
        .period_bounds()
        .unwrap()
        .0;
    let closure = layer2_core::budget::Closure {
        reason: layer2_core::budget::ClosureReason::SpendStop,
        since: 100,
        settings_revision: 1,
        committed_at_entry: 0,
    };
    store.model_budget.update_one(doc! {"_id": crate::store::budget::period_key(start)}, doc! {"$push": {"record.closures": mongodb::bson::serialize_to_bson(&closure).unwrap()}}).await.unwrap();
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "spent"))
            .await
            .unwrap_err(),
        CommandLedgerError::Budget(layer2_core::budget::BudgetError::PlatformExhausted)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_does_not_prevent_cancelling_an_already_queued_response() {
    let (client, name) = database("operator_cancel").await;
    let (store, _, _, request, source) =
        super::queued_cancellation_tests::queued(&name, &client).await;
    stop(&client, &name).await;
    let QueuedCancellationOutcome::Applied(cancelled) =
        store.cancel_queued_response(request.clone()).await.unwrap()
    else {
        panic!("queued cancellation expected")
    };
    assert_eq!(cancelled.response.status, AgentResponseStatus::Cancelled);
    assert_eq!(cancelled.response.receipt, source.receipt);
    assert!(
        matches!(store.cancel_queued_response(request).await.unwrap(), QueuedCancellationOutcome::Applied(replay) if replay == cancelled)
    );
    client.database(&name).drop().await.unwrap();
}
