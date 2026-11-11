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
fn platform_spend_pause_is_not_eligible_with_an_active_turn_or_queued_messages() {
    let current = sandbox();
    let mut busy = activity("agt_a", 4);
    busy.active_turn = Some(4);
    let mut queued = activity("agt_b", 8);
    queued.queued_messages_present = true;
    for activities in [vec![busy], vec![queued], vec![]] {
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
