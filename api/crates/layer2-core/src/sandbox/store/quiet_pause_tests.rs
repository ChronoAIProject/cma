use super::*;
use crate::sandbox::lifecycle_test_support::{agent, log, sandbox, NOW};

fn quiet_agents() -> Vec<SandboxAgentActivity> {
    ["agt_a", "agt_b"]
        .iter()
        .map(|id| {
            SandboxAgentActivity::from_live_agent(
                &agent(id, &format!("conversation-{id}"), 4),
                Some(&log(id, 4, NOW)),
            )
            .unwrap()
        })
        .collect()
}

fn scheduled(activities: &[SandboxAgentActivity]) -> bool {
    let sandbox = sandbox();
    matches!(
        apply_consented_quiet_pause(&sandbox, activities, &sandbox.owner, NOW).unwrap(),
        SandboxMaintenanceApplication::Persist { .. }
    )
}

#[test]
fn explicit_consent_can_pause_immediately_after_the_last_turn_finishes() {
    let sandbox = sandbox();
    let SandboxMaintenanceApplication::Persist {
        expected_revision,
        sandbox: next,
        event,
    } = apply_consented_quiet_pause(&sandbox, &quiet_agents(), &sandbox.owner, NOW).unwrap()
    else {
        panic!("owner-consented quiet pause must not wait for inactivity timeout")
    };
    assert_eq!(expected_revision, sandbox.revision);
    assert_eq!(next.status, SandboxStatus::Pausing);
    assert_eq!(next.runtime, sandbox.runtime);
    assert_eq!(next.credential, sandbox.credential);
    assert_eq!(
        event,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        }
    );
}

#[test]
fn any_sibling_turn_work_or_queued_input_prevents_pause() {
    let mutations: Vec<fn(&mut SandboxAgentActivity)> = vec![
        |a| a.status = AgentStatus::Working,
        |a| a.active_turn = Some(5),
        |a| a.work_pending = true,
        |a| a.queued_messages_present = true,
        |a| a.stop_requested = true,
    ];
    for mutate in mutations {
        let mut activities = quiet_agents();
        mutate(&mut activities[1]);
        assert!(!scheduled(&activities));
    }
}

#[test]
fn partial_duplicate_or_wrong_identity_evidence_is_refused() {
    assert!(!scheduled(&[]));
    let mut duplicates = quiet_agents();
    duplicates.push(duplicates[0].clone());
    assert!(!scheduled(&duplicates));
    let mutations: Vec<fn(&mut SandboxAgentActivity)> = vec![
        |a| a.sandbox_id.0 = "another-sandbox".into(),
        |a| a.conversation_id = None,
        |a| a.runtime_generation = None,
        |a| a.last_seq = 0,
        |a| a.newest_log = None,
        |a| a.newest_log.as_mut().unwrap().seq = 3,
        |a| a.newest_log.as_mut().unwrap().agent_id.0 = "another-agent".into(),
        |a| a.newest_log.as_mut().unwrap().appended_at = "2026-09-06T00:00:00Z".into(),
    ];
    for mutate in mutations {
        let mut activities = quiet_agents();
        mutate(&mut activities[0]);
        assert!(!scheduled(&activities));
    }
}

#[test]
fn consent_must_match_owner_and_cannot_overlap_an_existing_lifecycle() {
    let mut sandbox = sandbox();
    assert!(apply_consented_quiet_pause(
        &sandbox,
        &quiet_agents(),
        &SubjectId("other".into()),
        NOW
    )
    .is_err());
    assert!(
        apply_consented_quiet_pause(&sandbox, &quiet_agents(), &sandbox.owner, "invalid").is_err()
    );
    sandbox.begin_pause(NOW.into(), None).unwrap();
    assert_eq!(
        apply_consented_quiet_pause(&sandbox, &quiet_agents(), &sandbox.owner, NOW).unwrap(),
        SandboxMaintenanceApplication::Complete(SandboxMaintenanceOutcome::NotEligible),
    );
}
