use super::*;
use layer2_core::sandbox::store::{
    DesktopSessionRequest, InactivityPauseCause, SandboxSessionStore,
};
use mongodb::bson::{doc, Bson, Document};

use crate::store::sandbox_maintenance::evidence::sessions::MAX_LEASE_ROWS;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn strict_desktop_evidence_distinguishes_expiry_from_missing_proof() {
    let (client, name, bypass) = database().await;
    let store = bypass.clone().with_strict_inactivity_session_evidence(true);
    let (_, policy) = policy_tests::enable(&client, &name).await;
    for case in [
        "no_rows",
        "missing",
        "wrong_type",
        "wrong_parent_type",
        "malformed",
        "mixed",
        "legacy_future",
        "legacy_expired",
        "legacy_extra_field",
        "current_expired",
        "unsupported",
        "partial_exclusive",
        "cap",
        "overflow",
        "terminal_agents",
    ] {
        let id = format!("sbx_strict_{case}");
        let sandbox = if case == "terminal_agents" {
            let sandbox = active_sandbox(&id);
            store.create_sandbox(sandbox.clone()).await.unwrap();
            let mut stopped = ready_agent(&sandbox, "agt_terminal");
            stopped
                .transition(
                    AgentStatus::Stopped,
                    IDLE_AT.into(),
                    Some(layer2_core::sandbox::AgentStopReason::Owner),
                )
                .unwrap();
            insert_agent(&store, &stopped, IDLE_AT).await;
            sandbox
        } else {
            fixture(&store, &id).await.0
        };
        let request = SandboxInactivityPauseRequest {
            expected_policy_revision: Some(policy.revision),
            ..policy_tests::global(&sandbox.id)
        };
        if case == "terminal_agents" {
            assert_eq!(
                store
                    .preview_inactivity_pause(request.clone())
                    .await
                    .unwrap(),
                SandboxMaintenanceOutcome::Scheduled
            );
        }
        let expired = Bson::String(LATER.into());
        let count = match case {
            "no_rows" => 0,
            "mixed" => 2,
            "cap" => MAX_LEASE_ROWS,
            "overflow" => MAX_LEASE_ROWS + 1,
            _ => 1,
        };
        let rows: Vec<Document> = (0..count)
            .map(|n| {
                let mut row = doc! { "_id": format!("{id}_{n}"), "sandbox_id": &id };
                match case {
                    "missing" | "terminal_agents" => {}
                    "wrong_type" => {
                        row.insert("lease_expires_at", 9_i64);
                    }
                    "wrong_parent_type" => {
                        row.insert("sandbox_id", vec![id.clone()]);
                        row.insert("lease_expires_at", expired.clone());
                    }
                    "malformed" => {
                        row.insert("lease_expires_at", "0000-99-99T00:00:00Z");
                    }
                    "mixed" if n == 1 => {
                        row.insert("lease_expires_at", "0001");
                    }
                    "legacy_future" => {
                        row.insert("lease_expires_at", "2026-09-05T01:00:00Z");
                    }
                    _ => {
                        row.insert("lease_expires_at", expired.clone());
                    }
                }
                if matches!(
                    case,
                    "current_expired" | "partial_exclusive" | "unsupported"
                ) {
                    row.insert(
                        "document_schema_version",
                        DOCUMENT_SCHEMA_VERSION + i64::from(case == "unsupported"),
                    );
                }
                if case == "partial_exclusive" {
                    row.insert("epoch", 1_i64);
                }
                if case == "legacy_extra_field" {
                    row.insert("unrecognized", true);
                }
                row
            })
            .collect();
        if !rows.is_empty() {
            store.sandbox_sessions.insert_many(rows).await.unwrap();
        }
        let eligible = matches!(
            case,
            "no_rows" | "legacy_expired" | "current_expired" | "cap"
        );
        let wanted = if eligible {
            SandboxMaintenanceOutcome::Scheduled
        } else {
            SandboxMaintenanceOutcome::NotEligible
        };
        assert_eq!(
            store
                .preview_inactivity_pause(request.clone())
                .await
                .unwrap(),
            wanted,
            "{case}"
        );
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone()),
            "{case}"
        );
        assert!(store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store.schedule_inactivity_pause(request).await.unwrap(),
            wanted,
            "{case}"
        );
        if !eligible {
            assert_eq!(
                store.get_sandbox(&sandbox.id).await.unwrap(),
                Some(sandbox.clone()),
                "{case}"
            );
            assert!(store
                .sandbox_events(&sandbox.id, 0, 10)
                .await
                .unwrap()
                .is_empty());
        }
        if case == "missing" {
            assert_eq!(
                bypass
                    .preview_inactivity_pause(policy_tests::global(&sandbox.id))
                    .await
                    .unwrap(),
                SandboxMaintenanceOutcome::Scheduled
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn strict_operator_rechecks_exclusive_lease_against_mongo_time() {
    let (client, name, store) = database().await;
    let store = store.with_strict_inactivity_session_evidence(true);
    let (_, policy) = policy_tests::enable(&client, &name).await;
    let (sandbox, _) = fixture(&store, "sbx_time_skew").await;
    let request = SandboxInactivityPauseRequest {
        requested_at: "2099-09-05T00:30:00Z".into(),
        expected_policy_revision: Some(policy.revision),
        ..policy_tests::global(&sandbox.id)
    };
    assert_eq!(
        store
            .preview_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let authority = store
        .acquire_exclusive_sandbox_session(&DesktopSessionRequest {
            sandbox_id: sandbox.id.clone(),
            session_id: "synthetic".into(),
            runtime: sandbox.runtime.clone().unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                cause: InactivityPauseCause::OperatorInactivity,
                ..request.clone()
            })
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert!(store
        .sandbox_events(&sandbox.id, 0, 10)
        .await
        .unwrap()
        .is_empty());

    // A parseable old string must not hide a different exclusive deadline.
    store
        .sandbox_sessions
        .update_one(
            doc! {"_id": &authority.session_id},
            doc! {"$set": {"lease_expires_at": IDLE_AT}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.preview_inactivity_pause(request).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    platform_stop_integration_tests::install_stop(&store).await;
    let spend_stop = SandboxInactivityPauseRequest {
        cause: InactivityPauseCause::PlatformSpendStop,
        pause_after_seconds: 0,
        ..policy_tests::global(&sandbox.id)
    };
    // This controls which branch is selected independently of the document shape.
    assert_eq!(
        store.preview_inactivity_pause(spend_stop).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    client.database(&name).drop().await.unwrap();
}
