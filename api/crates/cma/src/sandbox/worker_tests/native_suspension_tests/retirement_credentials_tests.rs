use super::*;

fn arm_gc(f: &Fixture) {
    *f.backend.gc_credentials.lock().unwrap() = Some(f.harness.credentials.clone());
}

fn add_debt(f: &Fixture, row: &mut Sandbox, now: SystemTime) {
    row.credential
        .queue_revoke("older-key".into(), rfc3339_at(now));
    f.harness
        .credentials
        .live_keys
        .lock()
        .unwrap()
        .insert("older-key".into());
    f.harness
        .credentials
        .revoke_failure_ids
        .lock()
        .unwrap()
        .insert("older-key".into());
}

#[tokio::test]
async fn retained_fast_paths_settle_parent_debt_before_provider_gc_even_with_lost_reply() {
    for state in ["suspended", "terminating", "absent", "uncredentialed"] {
        let (f, mut original, mut now) = if state == "uncredentialed" {
            let (f, row, _, now) = retirement_tests::failed_revive().await;
            (f.without_retirement_admission(), row, now)
        } else {
            // The production default: finality admission off keeps this path.
            let f = Fixture::new().await.without_retirement_admission();
            let created = f.harness.create().await;
            let mut now = start_time(&created);
            let active = f
                .drive(
                    &created.id,
                    &mut now,
                    SandboxStatus::Active,
                    &runtime_image(),
                )
                .await;
            let paused = f.pause(&active, &mut now).await;
            (f, paused, now)
        };
        if state == "terminating" || state == "absent" {
            original.runtime = original
                .lifecycle
                .suspended_runtime
                .take()
                .map(|binding| *binding);
            if state == "absent" {
                f.backend.lose_object().await;
            } else {
                f.backend.set_lifecycle(state).await;
            }
        }
        let revision = original.revision;
        original.begin_retirement(rfc3339_at(now)).unwrap();
        add_debt(&f, &mut original, now);
        f.harness
            .store
            .save_sandbox(revision, original.clone())
            .await
            .unwrap();
        arm_gc(&f);
        f.backend.lose_delete.store(true, Ordering::SeqCst);
        f.step(&original.id, now, &runtime_image()).await;
        let blocked = f.harness.stored(&original.id).await;
        assert_eq!(blocked.status, SandboxStatus::Retiring, "{state}");
        assert_eq!(blocked.work.step, SandboxWorkStep::StopAgents);
        assert!(!blocked.lifecycle.home.as_ref().unwrap().released);
        assert!(!f
            .backend
            .calls
            .snapshot()
            .contains(&"delete-retained".into()));
        assert!(!f.backend.home_destroyed.load(Ordering::SeqCst));
        assert_eq!(blocked.credential.revoke_debt.len(), 1);
        assert!(stamp_time(&blocked.work.ready_at) > now);
        f.harness
            .credentials
            .revoke_failure_ids
            .lock()
            .unwrap()
            .clear();
        let retired = f
            .drive(
                &original.id,
                &mut now,
                SandboxStatus::Retired,
                &runtime_image(),
            )
            .await;
        assert!(retired.lifecycle.home.as_ref().unwrap().released);
        assert!(retired.credential.revoke_debt.is_empty());
        if state != "absent" {
            assert!(f.backend.home_destroyed.load(Ordering::SeqCst));
        }
        assert_eq!(
            f.backend
                .calls
                .snapshot()
                .iter()
                .filter(|s| *s == "delete-retained")
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn older_cleanup_phases_hold_the_home_until_parent_only_debt_is_settled() {
    for step in [
        SandboxWorkStep::TeardownRuntime,
        SandboxWorkStep::MarkRetired,
    ] {
        let f = Fixture::native().await;
        let created = f.harness.create().await;
        let mut now = start_time(&created);
        let mut row = f
            .drive(
                &created.id,
                &mut now,
                SandboxStatus::Active,
                &runtime_image(),
            )
            .await;
        let revision = row.revision;
        row.begin_retirement(rfc3339_at(now)).unwrap();
        row.work.step = step;
        if step == SandboxWorkStep::MarkRetired {
            use layer3_adapter::hosted::HostedBackend;
            f.backend
                .teardown(&RuntimeHandle::new(
                    &row.runtime.as_ref().unwrap().runtime_id,
                ))
                .await
                .unwrap();
            row.runtime = None;
        }
        row.credential.current = None;
        row.credential.pending = None;
        row.credential.delivered = false;
        // Simulate a retained parent whose only remaining key has no custody row.
        f.harness.credentials.materials.lock().unwrap().clear();
        f.harness.credentials.live_keys.lock().unwrap().clear();
        add_debt(&f, &mut row, now);
        f.harness
            .store
            .save_sandbox(revision, row.clone())
            .await
            .unwrap();
        let calls = f.backend.calls.snapshot();
        arm_gc(&f);
        f.step(&row.id, now, &runtime_image()).await;
        let blocked = f.harness.stored(&row.id).await;
        assert_eq!(blocked.status, SandboxStatus::Retiring);
        assert_eq!(blocked.work.step, step);
        assert_eq!(f.backend.calls.snapshot(), calls);
        assert_eq!(blocked.lifecycle.home, row.lifecycle.home);
        assert!(!blocked.lifecycle.home.as_ref().unwrap().released);
        f.harness
            .credentials
            .revoke_failure_ids
            .lock()
            .unwrap()
            .clear();
        let retired = f
            .drive(&row.id, &mut now, SandboxStatus::Retired, &runtime_image())
            .await;
        assert!(retired.lifecycle.home.as_ref().unwrap().released);
        assert!(retired.credential.revoke_debt.is_empty());
    }
}

#[tokio::test]
async fn retirement_suspends_native_compute_when_the_final_issuance_check_is_uncertain() {
    let f = Fixture::native().await;
    let created = f.harness.create().await;
    let mut now = start_time(&created);
    let active = f
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let mut retiring = active.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    f.harness
        .store
        .save_sandbox(active.revision, retiring)
        .await
        .unwrap();
    f.step(&active.id, now, &runtime_image()).await;
    let stopped = f.harness.stored(&active.id).await;
    assert_eq!(
        stopped.work.step,
        SandboxWorkStep::RevokeRetirementCredentials
    );
    assert!(stopped.credential.current.is_some());

    use crate::sandbox::credential::CredentialError;
    *f.harness
        .credentials
        .late_settlement_failure
        .lock()
        .unwrap() = Some(CredentialError::Uncertain);
    now = stamp_time(&stopped.work.ready_at);
    f.backend.lose_suspend.store(true, Ordering::SeqCst);
    f.step(&active.id, now, &runtime_image()).await;
    let unconfirmed = f.harness.stored(&active.id).await;
    assert_eq!(unconfirmed.runtime, active.runtime);
    assert!(unconfirmed.lifecycle.suspended_runtime.is_none());
    assert_eq!(unconfirmed.lifecycle.home, active.lifecycle.home);
    assert!(unconfirmed.work.lease.is_none());

    now = stamp_time(&unconfirmed.work.ready_at);
    f.step(&active.id, now, &runtime_image()).await;
    let suspended = f.harness.stored(&active.id).await;
    assert_eq!(suspended.status, SandboxStatus::Retiring);
    assert_eq!(
        suspended.work.step,
        SandboxWorkStep::RevokeRetirementCredentials
    );
    assert!(suspended.runtime.is_none());
    assert_eq!(
        suspended.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    assert_eq!(suspended.lifecycle.home, active.lifecycle.home);
    assert_eq!(suspended.credential, stopped.credential);
    assert!(suspended.retired_at.is_none());
    let calls = f.backend.calls.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "suspend")
            .count(),
        1
    );
    assert!(!calls.iter().any(|call| call == "delete-retained"));
    assert!(!f.backend.home_destroyed.load(Ordering::SeqCst));

    *f.harness
        .credentials
        .late_settlement_failure
        .lock()
        .unwrap() = None;
    now = stamp_time(&suspended.work.ready_at);
    let retired = f
        .drive(
            &active.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(retired.lifecycle.suspended_runtime.is_none());
}

#[tokio::test]
async fn retirement_suspends_a_proved_empty_native_replacement_before_stopping_agents() {
    let (f, mut row, _, now) = retirement_tests::failed_revive().await;
    let revision = row.revision;
    row.begin_retirement(rfc3339_at(now)).unwrap();
    f.harness
        .store
        .save_sandbox(revision, row.clone())
        .await
        .unwrap();
    use crate::sandbox::credential::CredentialError;
    *f.harness.credentials.settlement_failure.lock().unwrap() = Some(CredentialError::Uncertain);

    f.step(&row.id, now, &runtime_image()).await;
    let blocked = f.harness.stored(&row.id).await;
    assert_eq!(blocked.status, SandboxStatus::Retiring);
    assert_eq!(blocked.work.step, SandboxWorkStep::StopAgents);
    assert!(blocked.runtime.is_none());
    assert_eq!(
        blocked.lifecycle.suspended_runtime.as_deref(),
        row.runtime.as_ref()
    );
    assert_eq!(blocked.lifecycle.home, row.lifecycle.home);
    assert_eq!(blocked.credential, row.credential);
    assert!(!f.backend.home_destroyed.load(Ordering::SeqCst));
}
