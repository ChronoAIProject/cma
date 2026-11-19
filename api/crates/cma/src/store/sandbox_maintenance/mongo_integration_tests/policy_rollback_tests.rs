use super::platform_stop_integration_tests::workspace;
use super::*;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;
use futures_util::TryStreamExt;
use layer2_core::PlatformSettingsStore;
use mongodb::bson::Document;
use std::time::Duration;

async fn ready_fixture(store: &MongoSandboxStore, id: &str) -> (Sandbox, Agent) {
    let (sandbox, agent) = fixture(store, id).await;
    let workspace = workspace(&sandbox, WorkspaceWorkStep::None);
    assert_eq!(workspace.id, agent.workspace_id);
    store.create_workspace(workspace).await.unwrap();
    store
        .append_sandbox_event(&sandbox.id, IDLE_AT.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    (sandbox, agent)
}

async fn snapshot(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    agent: &Agent,
) -> Vec<(&'static str, Vec<Document>)> {
    let mut rows = Vec::new();
    for (name, collection, filter) in [
        (
            "sandbox",
            &store.sandboxes,
            doc! {"_id":sandbox.id.as_str()},
        ),
        (
            "agents",
            &store.agents,
            doc! {"sandbox_id":sandbox.id.as_str()},
        ),
        (
            "workspaces",
            &store.workspaces,
            doc! {"sandbox_id":sandbox.id.as_str()},
        ),
        (
            "events",
            &store.sandbox_events,
            doc! {"sandbox_id":sandbox.id.as_str()},
        ),
        (
            "usage",
            &store.sandbox_usage,
            doc! {"_id":sandbox.id.as_str()},
        ),
        (
            "usage_events",
            &store.sandbox_usage_events,
            doc! {"sandbox_id":sandbox.id.as_str()},
        ),
        (
            "agent_log",
            &store.agent_log,
            doc! {"agent_id":agent.id.as_str()},
        ),
    ] {
        rows.push((
            name,
            collection
                .find(filter)
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
        ));
    }
    for name in [
        "sandbox",
        "agents",
        "workspaces",
        "events",
        "usage",
        "usage_events",
        "agent_log",
    ] {
        assert!(
            !rows
                .iter()
                .find(|(key, _)| *key == name)
                .unwrap()
                .1
                .is_empty(),
            "{name}"
        );
    }
    rows
}

fn global(sandbox: &Sandbox, revision: u64) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        auto_pause_enabled: true,
        expected_policy_revision: Some(revision),
        ..pause(&sandbox.id)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_policy_changes_abort_every_candidate_row_before_retrying() {
    for change in ["disable", "threshold", "grace"] {
        let (client, name, store) = database().await;
        let (settings, policy) = policy_tests::enable(&client, &name).await;
        let (sandbox, agent) = ready_fixture(&store, &format!("sbx_late_policy_{change}")).await;
        let before = snapshot(&store, &sandbox, &agent).await;
        let replica_client =
            Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                .await
                .unwrap();
        let replica = MongoSandboxStore::open(&replica_client, &name)
            .await
            .unwrap();
        let mut updated = policy.settings.clone();
        match change {
            "disable" => updated.inactivity.auto_pause_enabled = false,
            "threshold" => updated.inactivity.pause_after_seconds = 3600,
            _ => updated.inactivity.grace_started_at = Some(LATER.into()),
        }
        let filter = doc! {
            "_id":"platform", "document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "revision":i64::try_from(policy.revision).unwrap(),
        };
        let update = doc! {
            "$set":{"settings":mongodb::bson::serialize_to_bson(&updated).unwrap()},
            "$inc":{"revision":1_i64},
        };
        let mut writer = client.start_session().await.unwrap();
        start_transaction(&mut writer).await.unwrap();
        let gate = Arc::new(super::super::admission_test_gate::Gate::default());
        let candidate_gate = gate.clone();
        let request = global(&sandbox, policy.revision);
        let mut candidate = tokio::spawn(async move {
            candidate_gate
                .scope(replica.schedule_inactivity_pause(request))
                .await
        });
        let reached = tokio::time::timeout(Duration::from_secs(10), gate.reached()).await;
        // Commit a real revision CAS while the candidate's local writes are
        // tentative. Its final settings write must conflict with this snapshot.
        let changed = if reached.is_ok() {
            Some(
                store
                    .budget_settings
                    .update_one(filter, update)
                    .session(&mut writer)
                    .await,
            )
        } else {
            None
        };
        let committed = match &changed {
            Some(Ok(result)) if result.matched_count == 1 => {
                let committed = writer.commit_transaction().await;
                if committed.is_err() {
                    let _ = writer.abort_transaction().await;
                }
                Some(committed)
            }
            _ => {
                let _ = writer.abort_transaction().await;
                None
            }
        };
        gate.release();
        let outcome = match tokio::time::timeout(Duration::from_secs(10), &mut candidate).await {
            Ok(outcome) => outcome,
            Err(_) => {
                candidate.abort();
                candidate.await
            }
        };
        let after = snapshot(&store, &sandbox, &agent).await;
        let final_policy = settings.read().await.unwrap();
        let fence = store
            .budget_settings
            .find_one(doc! {"_id":"platform"})
            .await
            .unwrap()
            .unwrap()
            .get_i64("inactivity_admission_fence")
            .unwrap_or(0);
        client.database(&name).drop().await.unwrap();

        assert!(
            reached.is_ok(),
            "candidate did not finish local persistence: {change}"
        );
        assert!(
            matches!(changed, Some(Ok(result)) if result.matched_count == 1),
            "{change}"
        );
        assert!(matches!(committed, Some(Ok(()))), "{change}");
        let result_label = match &outcome {
            Ok(Ok(value)) => format!("{value:?}"),
            Ok(Err(error)) => super::contention_tests::diagnostics::classify_error(error).into(),
            Err(_) => "task_failed".into(),
        };
        assert!(
            matches!(outcome, Ok(Ok(SandboxMaintenanceOutcome::NotEligible))),
            "{change}: candidate_result={result_label}"
        );
        assert_eq!(
            after, before,
            "every tentative row must roll back: {change}"
        );
        assert_eq!(final_policy.revision, policy.revision + 1);
        assert_eq!(final_policy.settings, updated);
        assert_eq!(
            fence, 0,
            "a rejected attempt cannot commit its policy fence"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_preview_aborts_every_candidate_row_and_the_final_policy_fence() {
    let (client, name, store) = database().await;
    let (_, policy) = policy_tests::enable(&client, &name).await;
    let (sandbox, agent) = ready_fixture(&store, "sbx_global_preview_rows").await;
    let before = snapshot(&store, &sandbox, &agent).await;
    let policy_before = store
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap();
    let outcome = store
        .preview_inactivity_pause(global(&sandbox, policy.revision))
        .await;
    let after = snapshot(&store, &sandbox, &agent).await;
    let policy_after = store
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();

    let error = outcome
        .as_ref()
        .err()
        .map(super::contention_tests::diagnostics::classify_error)
        .unwrap_or("none");
    assert!(
        matches!(outcome, Ok(SandboxMaintenanceOutcome::Scheduled)),
        "preview_error={error}"
    );
    assert_eq!(after, before);
    assert_eq!(policy_after, policy_before);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn disabled_policy_still_precedes_corrupt_event_history() {
    let (client, name, store) = database().await;
    let (settings, policy) = policy_tests::enable(&client, &name).await;
    let (sandbox, agent) = ready_fixture(&store, "sbx_policy_error_priority").await;
    store
        .sandbox_events
        .update_one(
            doc! {"sandbox_id":sandbox.id.as_str(), "seq":1_i64},
            doc! {"$set":{"record":"corrupt event"}},
        )
        .await
        .unwrap();
    let mut disabled = policy.settings.clone();
    disabled.inactivity.auto_pause_enabled = false;
    let disabled = settings.replace(policy.revision, &disabled).await.unwrap();
    let before = snapshot(&store, &sandbox, &agent).await;
    let rejected = store
        .schedule_inactivity_pause(global(&sandbox, disabled.revision))
        .await;
    assert_eq!(snapshot(&store, &sandbox, &agent).await, before);
    let enabled = settings
        .replace(disabled.revision, &policy.settings)
        .await
        .unwrap();
    let corrupt = store
        .schedule_inactivity_pause(global(&sandbox, enabled.revision))
        .await;
    let after = snapshot(&store, &sandbox, &agent).await;
    client.database(&name).drop().await.unwrap();

    let error = rejected
        .as_ref()
        .err()
        .map(super::contention_tests::diagnostics::classify_error)
        .unwrap_or("none");
    assert!(
        matches!(rejected, Ok(SandboxMaintenanceOutcome::NotEligible)),
        "policy_error={error}"
    );
    assert!(matches!(corrupt, Err(SandboxStoreError::Corrupt(_))));
    assert_eq!(after, before);
}
