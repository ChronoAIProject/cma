use super::*;
use crate::sandbox::lifecycle_test_support::{agent, log, sandbox};
use crate::sandbox::SandboxAgentActivity;

/// The newest log is ten seconds old: well under any inactivity threshold.
const RECENT: &str = "2026-09-05T00:59:50Z";
const AT: &str = "2026-09-05T01:00:00Z";
const PLATFORM_NOTICE: &str = "Platform spend limit reached. Your work is saved; the Sandbox will be available again after the limit is raised.";

fn activity(agent_id: &str, seq: u64) -> SandboxAgentActivity {
    let agent = agent(agent_id, &format!("conversation-{agent_id}"), seq);
    let log = log(agent_id, seq, RECENT);
    SandboxAgentActivity::from_live_agent(&agent, Some(&log)).unwrap()
}

fn request(cause: InactivityPauseCause, pause_after_seconds: u64) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        sandbox_id: sandbox().id,
        requested_at: AT.into(),
        pause_after_seconds,
        auto_pause_enabled: true,
        expected_policy_revision: None,
        cause,
    }
}

fn scheduled_notice(application: SandboxMaintenanceApplication) -> Option<String> {
    match application {
        SandboxMaintenanceApplication::Persist { sandbox, .. } => sandbox.lifecycle.paused_notice,
        SandboxMaintenanceApplication::Complete(_) => None,
    }
}

#[test]
fn platform_spend_pause_ignores_the_inactivity_threshold_and_uses_the_platform_notice() {
    let current = sandbox();
    let quiet = vec![activity("agt_a", 4)];
    assert_eq!(
        apply_inactivity_pause(
            &current,
            &quiet,
            &request(InactivityPauseCause::Inactivity, 60)
        ),
        Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible
        ))
    );
    assert!(apply_inactivity_pause(
        &current,
        &quiet,
        &request(InactivityPauseCause::PlatformSpendStop, 60)
    )
    .is_err());
    let application = apply_inactivity_pause(
        &current,
        &quiet,
        &request(InactivityPauseCause::PlatformSpendStop, 0),
    )
    .unwrap();
    let SandboxMaintenanceApplication::Persist {
        sandbox: paused, ..
    } = application.clone()
    else {
        panic!("a quiet Sandbox is paused under a platform stop");
    };
    assert_eq!(paused.status, SandboxStatus::Pausing);
    assert_eq!(
        scheduled_notice(application).as_deref(),
        Some(PLATFORM_NOTICE)
    );
    assert_eq!(
        apply_inactivity_pause(
            &paused,
            &quiet,
            &request(InactivityPauseCause::PlatformSpendStop, 0)
        ),
        Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::AlreadyScheduled
        ))
    );
    assert!(apply_inactivity_pause(
        &current,
        &quiet,
        &request(InactivityPauseCause::Inactivity, 0)
    )
    .is_err());
}

#[test]
fn platform_spend_pause_is_not_eligible_with_an_active_turn_queued_messages_or_pending_work() {
    let current = sandbox();
    let mut busy = activity("agt_a", 4);
    busy.active_turn = Some(4);
    let mut queued = activity("agt_b", 8);
    queued.queued_messages_present = true;
    let mut pending = activity("agt_c", 2);
    pending.work_pending = true;
    let mut admitted = activity("agt_d", 3);
    admitted.external_activity_present = true;
    for activities in [
        vec![busy],
        vec![queued],
        vec![pending],
        vec![activity("agt_e", 5), admitted],
    ] {
        assert_eq!(
            apply_inactivity_pause(
                &current,
                &activities,
                &request(InactivityPauseCause::PlatformSpendStop, 0)
            ),
            Ok(SandboxMaintenanceApplication::Complete(
                SandboxMaintenanceOutcome::NotEligible
            ))
        );
    }
}

#[test]
fn platform_spend_pause_admits_a_sandbox_without_live_agents_and_inactivity_does_not() {
    let current = sandbox();
    assert_eq!(
        apply_inactivity_pause(
            &current,
            &[],
            &request(InactivityPauseCause::Inactivity, 60)
        ),
        Ok(SandboxMaintenanceApplication::Complete(
            SandboxMaintenanceOutcome::NotEligible
        ))
    );
    let application = apply_inactivity_pause(
        &current,
        &[],
        &request(InactivityPauseCause::PlatformSpendStop, 0),
    )
    .unwrap();
    let SandboxMaintenanceApplication::Persist {
        sandbox: paused, ..
    } = application.clone()
    else {
        panic!("an Active Sandbox without live Agents is paused under a platform stop");
    };
    assert_eq!(paused.status, SandboxStatus::Pausing);
    assert_eq!(
        scheduled_notice(application).as_deref(),
        Some(PLATFORM_NOTICE)
    );
    // Only a bound, Active Sandbox is a candidate without live Agents.
    let mut unbound = sandbox();
    unbound.runtime = None;
    let mut paused_already = sandbox();
    paused_already.status = SandboxStatus::Paused;
    for other in [unbound, paused_already] {
        assert!(!crate::sandbox::is_platform_stop_pause_candidate(
            &other,
            &[],
            AT
        ));
    }
    assert!(crate::sandbox::is_platform_stop_pause_candidate(
        &current,
        &[],
        AT
    ));
    assert!(!crate::sandbox::is_platform_stop_pause_candidate(
        &current,
        &[],
        "not a time"
    ));
}

#[test]
fn operator_pause_bypasses_only_grace_and_requires_known_enabled_matching_policy() {
    let mut stored = crate::StoredPlatformSettings {
        changes: Default::default(),
        revision: 7,
        settings: Default::default(),
    };
    stored.settings.inactivity.auto_pause_enabled = true;
    stored.settings.inactivity.grace_started_at = Some(AT.into());
    let ordinary = request(InactivityPauseCause::Inactivity, 900);
    assert!(!ordinary.matches_policy(&stored));
    let operator = request(InactivityPauseCause::OperatorInactivity, 900);
    assert!(operator.matches_policy(&stored));
    stored.settings.inactivity.auto_pause_enabled = false;
    assert!(!operator.matches_policy(&stored));
    stored.settings.inactivity.auto_pause_enabled = true;
    stored.settings.inactivity.pause_after_seconds = 901;
    assert!(!operator.matches_policy(&stored));
    stored.settings.inactivity.pause_after_seconds = 900;
    stored.settings.inactivity.grace_started_at = None;
    assert!(!operator.matches_policy(&stored));
    stored.settings.inactivity.grace_started_at = Some("2026-09-05T00:00:00Z".into());
    assert!(ordinary.matches_policy(&stored));
    stored.revision = 0;
    assert!(!ordinary.matches_policy(&stored));
}
