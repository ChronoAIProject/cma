use super::*;
use layer2_core::{PlatformSettingsStore, StoredPlatformSettings};

pub(super) async fn enable(
    client: &Client,
    name: &str,
) -> (
    crate::store::settings::MongoPlatformSettingsStore,
    StoredPlatformSettings,
) {
    let settings = crate::store::settings::MongoPlatformSettingsStore::open(client, name)
        .await
        .unwrap();
    let current = settings.read().await.unwrap();
    let mut enabled = current.settings;
    enabled.inactivity.auto_pause_enabled = true;
    enabled.inactivity.grace_started_at = Some(IDLE_AT.into());
    let stored = settings.replace(current.revision, &enabled).await.unwrap();
    (settings, stored)
}

fn global(id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        auto_pause_enabled: true,
        expected_policy_revision: None,
        ..pause(id)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_policy_revision_fences_disable_threshold_and_grace_races() {
    for change in ["disable", "threshold", "grace"] {
        let (client, name, store) = database().await;
        let (sandbox, ready) = fixture(&store, "sbx_policy_race").await;
        let (settings, mut policy) = enable(&client, &name).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        evidence::live_activities(&store, &[ready], &mut session, LATER)
            .await
            .unwrap();
        match change {
            "disable" => policy.settings.inactivity.auto_pause_enabled = false,
            "threshold" => policy.settings.inactivity.pause_after_seconds = 3600,
            _ => policy.settings.inactivity.grace_started_at = Some(LATER.into()),
        }
        settings
            .replace(policy.revision, &policy.settings)
            .await
            .unwrap();
        let result = transaction::apply(
            &store,
            &MaintenanceRequest::Inactivity(global(&sandbox.id)),
            &mut session,
        )
        .await;
        assert!(
            matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)),
            "{change}"
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .schedule_inactivity_pause(global(&sandbox.id))
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible
        );
        assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_preview_aborts_fences_and_missing_disabled_or_unknown_policy_fails_closed() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_policy_preview").await;
    assert_eq!(
        store
            .schedule_inactivity_pause(global(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let (settings, policy) = enable(&client, &name).await;
    let before = store
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .preview_inactivity_pause(global(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        store
            .budget_settings
            .find_one(doc! {"_id":"platform"})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(settings.read().await.unwrap(), policy);
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store
            .schedule_inactivity_pause(global(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let after = store
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.get_i64("inactivity_admission_fence").unwrap(), 1);
    assert_eq!(settings.read().await.unwrap(), policy);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_bypasses_only_grace_and_refuses_stale_or_corrupt_policy() {
    use layer2_core::sandbox::store::InactivityPauseCause;
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_operator_policy").await;
    let (settings, mut policy) = enable(&client, &name).await;
    policy.settings.inactivity.grace_started_at = Some(LATER.into());
    let policy = settings
        .replace(policy.revision, &policy.settings)
        .await
        .unwrap();
    let ordinary = global(&sandbox.id);
    let operator = SandboxInactivityPauseRequest {
        cause: InactivityPauseCause::OperatorInactivity,
        expected_policy_revision: Some(policy.revision),
        ..ordinary.clone()
    };
    assert_eq!(
        store.preview_inactivity_pause(ordinary).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store
            .preview_inactivity_pause(operator.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let stale = SandboxInactivityPauseRequest {
        expected_policy_revision: Some(policy.revision - 1),
        ..operator.clone()
    };
    assert_eq!(
        store.schedule_inactivity_pause(stale).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let mut updated = policy.settings.clone();
    updated.inactivity.auto_pause_enabled = false;
    settings.replace(policy.revision, &updated).await.unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(operator.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    store
        .budget_settings
        .update_one(
            doc! {"_id":"platform"},
            doc! {"$set":{"settings.inactivity.pause_after_seconds":"unknown"}},
        )
        .await
        .unwrap();
    assert!(store.schedule_inactivity_pause(operator).await.is_err());
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    client.database(&name).drop().await.unwrap();
}
