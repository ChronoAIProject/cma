use super::lifecycle_test_support::{agent, log, sandbox, IDLE_AT, NOW};
use super::*;
use crate::ids::{AgentId, SandboxId};

fn activity(agent_id: &str, conversation_id: &str, seq: u64) -> SandboxAgentActivity {
    let agent = agent(agent_id, conversation_id, seq);
    let log = log(agent_id, seq, IDLE_AT);
    SandboxAgentActivity::from_live_agent(&agent, Some(&log)).unwrap()
}

fn quiet_pair() -> Vec<SandboxAgentActivity> {
    vec![
        activity("agt_a", "conversation-a", 4),
        activity("agt_b", "conversation-b", 8),
    ]
}

#[test]
fn exact_server_composed_pause_notice_uses_configured_minutes() {
    assert_eq!(
        inactivity_pause_notice(900),
        "Your sandbox has been idle for 15 minutes and was paused. Send a message to continue."
    );
    assert_eq!(
        inactivity_pause_notice(3_600),
        "Your sandbox has been idle for 60 minutes and was paused. Send a message to continue."
    );
}

#[test]
fn activity_projection_copies_no_log_or_message_content() {
    let agent = agent("agt_a", "conversation-a", 4);
    let log = log("agt_a", 4, IDLE_AT);
    let activity = SandboxAgentActivity::from_live_agent(&agent, Some(&log)).unwrap();
    let json = serde_json::to_string(&activity).unwrap();

    assert!(!json.contains("private transcript content"));
    assert!(!json.contains("Private title"));
    assert_eq!(activity.newest_log.unwrap().seq, 4);
}

#[test]
fn open_desktop_session_registry_blocks_maintenance_until_stream_closes() {
    let sandbox_id = SandboxId("sbx_desktop_registry_test".into());
    assert!(!has_open_desktop_session(&sandbox_id));
    register_open_desktop_session(&sandbox_id);
    assert!(has_open_desktop_session(&sandbox_id));
    unregister_open_desktop_session(&sandbox_id);
    assert!(!has_open_desktop_session(&sandbox_id));
}

#[test]
fn all_complete_live_agents_must_be_strictly_older_than_the_threshold() {
    let sandbox = sandbox();
    let activities = quiet_pair();

    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &activities,
        "2026-09-05T00:30:00Z",
        900,
    ));
    assert!(is_inactivity_pause_candidate(
        &sandbox,
        &activities,
        NOW,
        900,
    ));

    let mut one_recent = activities;
    one_recent[1].newest_log.as_mut().unwrap().appended_at = "2026-09-05T00:29:00Z".into();
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &one_recent,
        NOW,
        900,
    ));
}

#[test]
fn recent_sandbox_activity_defers_pause_even_when_live_agent_logs_are_old() {
    // The owner's Workspace/Agent creation is admitted on the Sandbox while
    // the existing Agents stay quiet with old logs (#1330).
    let mut sandbox = sandbox();
    let activities = quiet_pair();
    sandbox.last_activity_at = "2026-09-05T00:20:00Z".into();
    for (now, eligible) in [
        ("2026-09-05T00:34:59Z", false),
        // The boundary is strict, as for Agent logs.
        ("2026-09-05T00:35:00Z", false),
        ("2026-09-05T00:35:01Z", true),
    ] {
        assert_eq!(
            is_inactivity_pause_candidate(&sandbox, &activities, now, 900),
            eligible,
            "{now}"
        );
    }

    // Each kind of evidence must be idle: a recent Agent log still defers
    // after the Sandbox activity is old.
    let mut recent_log = activities.clone();
    recent_log[0].newest_log.as_mut().unwrap().appended_at = "2026-09-05T00:25:00Z".into();
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &recent_log,
        "2026-09-05T00:35:01Z",
        900,
    ));

    // Malformed Sandbox activity fails closed.
    sandbox.last_activity_at = "not-a-time".into();
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &activities,
        NOW,
        900
    ));
}

#[test]
fn platform_stop_pause_keeps_its_zero_threshold_with_recent_sandbox_activity() {
    let mut sandbox = sandbox();
    sandbox.last_activity_at = NOW.into();
    assert!(is_platform_stop_pause_candidate(
        &sandbox,
        &quiet_pair(),
        NOW
    ));
}

#[test]
fn active_turn_lifecycle_work_and_queued_input_each_prevent_pause() {
    let sandbox = sandbox();

    let mut active_turn = quiet_pair();
    active_turn[0].status = AgentStatus::Working;
    active_turn[0].active_turn = Some(9);
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &active_turn,
        NOW,
        900,
    ));

    let mut work_pending = quiet_pair();
    work_pending[0].work_pending = true;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &work_pending,
        NOW,
        900,
    ));

    let mut queued = quiet_pair();
    queued[0].queued_messages_present = true;
    assert!(!is_inactivity_pause_candidate(&sandbox, &queued, NOW, 900,));

    let mut external_activity = quiet_pair();
    external_activity[0].external_activity_present = true;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &external_activity,
        NOW,
        900,
    ));

    let mut open_session = quiet_pair();
    open_session[0].open_terminal_or_desktop_session = true;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &open_session,
        NOW,
        900,
    ));

    let mut stopping = quiet_pair();
    stopping[0].stop_requested = true;
    assert!(!is_inactivity_pause_candidate(
        &sandbox, &stopping, NOW, 900,
    ));
}

#[test]
fn missing_partial_or_mismatched_durable_evidence_fails_closed() {
    let sandbox = sandbox();

    let mut missing_log = quiet_pair();
    missing_log[0].newest_log = None;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &missing_log,
        NOW,
        900,
    ));

    let mut partial_identity = quiet_pair();
    partial_identity[0].runtime_generation = None;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &partial_identity,
        NOW,
        900,
    ));

    let mut wrong_agent = quiet_pair();
    wrong_agent[0].newest_log.as_mut().unwrap().agent_id = AgentId("agt_other".into());
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &wrong_agent,
        NOW,
        900,
    ));

    let mut wrong_sequence = quiet_pair();
    wrong_sequence[0].newest_log.as_mut().unwrap().seq -= 1;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &wrong_sequence,
        NOW,
        900,
    ));

    let mut bad_time = quiet_pair();
    bad_time[0].newest_log.as_mut().unwrap().appended_at = "yesterday".into();
    assert!(!is_inactivity_pause_candidate(
        &sandbox, &bad_time, NOW, 900,
    ));
}

#[test]
fn candidate_requires_active_bound_sandbox_and_unique_live_agent_rows() {
    let mut candidate_sandbox = sandbox();
    let activities = quiet_pair();

    candidate_sandbox.status = SandboxStatus::Pausing;
    assert!(!is_inactivity_pause_candidate(
        &candidate_sandbox,
        &activities,
        NOW,
        900,
    ));
    candidate_sandbox.status = SandboxStatus::Active;
    candidate_sandbox.runtime = None;
    assert!(!is_inactivity_pause_candidate(
        &candidate_sandbox,
        &activities,
        NOW,
        900,
    ));

    let sandbox = sandbox();
    assert!(!is_inactivity_pause_candidate(&sandbox, &[], NOW, 900));
    let duplicate = vec![activities[0].clone(), activities[0].clone()];
    assert!(!is_inactivity_pause_candidate(
        &sandbox, &duplicate, NOW, 900,
    ));
}

#[test]
fn only_ready_or_already_paused_live_agents_are_resumable_candidates() {
    let sandbox = sandbox();
    let mut activities = quiet_pair();
    activities[0].status = AgentStatus::Paused;
    assert!(is_inactivity_pause_candidate(
        &sandbox,
        &activities,
        NOW,
        900,
    ));

    activities[0].status = AgentStatus::Starting;
    assert!(!is_inactivity_pause_candidate(
        &sandbox,
        &activities,
        NOW,
        900,
    ));

    let mut terminal = agent("agt_terminal", "conversation-terminal", 1);
    terminal.status = AgentStatus::Stopped;
    let terminal_log = log("agt_terminal", 1, IDLE_AT);
    assert!(SandboxAgentActivity::from_live_agent(&terminal, Some(&terminal_log)).is_err());
}

#[test]
fn malformed_runtime_and_foreign_agent_evidence_refuse_both_pause_paths() {
    let activities = quiet_pair();
    let valid = sandbox();
    assert!(is_inactivity_pause_candidate(&valid, &activities, NOW, 900));
    assert!(is_platform_stop_pause_candidate(&valid, &[], NOW));

    for missing_id in [true, false] {
        let mut invalid = valid.clone();
        let runtime = invalid.runtime.as_mut().unwrap();
        if missing_id {
            runtime.runtime_id = " ".into();
        } else {
            runtime.runtime_version = " ".into();
        }
        assert!(!is_inactivity_pause_candidate(
            &invalid,
            &activities,
            NOW,
            900
        ));
        assert!(!is_platform_stop_pause_candidate(&invalid, &[], NOW));
    }
    let mut foreign = activities.clone();
    foreign[1].sandbox_id = SandboxId("sbx_other".into());
    assert!(!is_inactivity_pause_candidate(&valid, &foreign, NOW, 900));
    assert!(!is_inactivity_pause_candidate(&valid, &activities, NOW, 0));
}
