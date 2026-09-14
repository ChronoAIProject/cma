use super::*;
use crate::sandbox::lifecycle_test_support::{agent, log, sandbox, NOW};
use crate::sandbox::store::{
    AgentLogStore, AgentStore, SandboxEventStore, SandboxLifecycleExecutionStore,
    SandboxMutationGuard, SandboxStore,
};
use crate::sandbox::{AgentLogKind, AgentWorkStep, InMemorySandboxStore};

const BEFORE: &str = "2026-09-05T00:32:00Z";
const BOUNDARY: &str = "2026-09-05T00:32:01Z";

fn activity() -> SandboxAgentActivity {
    SandboxAgentActivity::from_live_agent(
        &agent("agt_quiet", "conversation-quiet", 1),
        Some(&log("agt_quiet", 1, NOW)),
    )
    .unwrap()
}

#[test]
fn periodic_quiet_debounce_uses_the_exact_inclusive_120_second_boundary() {
    let current = sandbox();
    let activity = activity();
    assert_eq!(
        validate_checkpoint_capture_quiet(&current, std::slice::from_ref(&activity), BEFORE),
        Err(SandboxStoreError::Conflict)
    );
    assert!(validate_checkpoint_capture_quiet(&current, &[activity], BOUNDARY).is_ok());
    assert!(validate_checkpoint_capture_quiet(&current, &[], BOUNDARY).is_ok());
}

#[test]
fn periodic_quiet_debounce_fails_closed_for_incomplete_recent_or_busy_evidence() {
    let current = sandbox();
    for case in 0..14 {
        let mut evidence = activity();
        match case {
            0 => evidence.newest_log = None,
            1 => evidence.newest_log.as_mut().unwrap().seq = 0,
            2 => evidence.last_seq += 1,
            3 => evidence.newest_log.as_mut().unwrap().agent_id.0 = "agt_other".into(),
            4 => evidence.sandbox_id.0 = "sbx_other".into(),
            5 => evidence.newest_log.as_mut().unwrap().appended_at = "invalid".into(),
            6 => evidence.newest_log.as_mut().unwrap().appended_at = "2099-01-01T00:00:00Z".into(),
            7 => evidence.newest_log.as_mut().unwrap().appended_at = BEFORE.into(),
            8 => evidence.active_turn = Some(1),
            9 => evidence.work_pending = true,
            10 => evidence.stop_requested = true,
            11 => evidence.queued_messages_present = true,
            12 => evidence.status = AgentStatus::Working,
            13 => evidence.status = AgentStatus::Starting,
            _ => unreachable!(),
        }
        assert_eq!(
            validate_checkpoint_capture_quiet(&current, &[evidence], BOUNDARY),
            Err(SandboxStoreError::Conflict),
            "case {case}"
        );
    }
    assert!(validate_checkpoint_capture_quiet(&current, &[activity()], "invalid").is_err());
    assert!(
        validate_checkpoint_capture_quiet(&current, &[activity(), activity()], BOUNDARY).is_err()
    );
}

#[test]
fn periodic_quiet_debounce_considers_every_live_agent_and_does_not_gate_manual_pause() {
    let mut current = sandbox();
    let mut recent = activity();
    recent.agent_id.0 = "agt_recent".into();
    recent.newest_log.as_mut().unwrap().agent_id = recent.agent_id.clone();
    recent.newest_log.as_mut().unwrap().appended_at = BEFORE.into();
    assert!(validate_checkpoint_capture_quiet(&current, &[activity(), recent], BOUNDARY).is_err());
    current.begin_pause(NOW.into(), None).unwrap();
    assert!(current.lifecycle.operation.is_some());
    assert!(current.lifecycle.checkpoint_capture.is_none());
}

#[tokio::test]
async fn memory_periodic_begin_enforces_durable_quiet_without_persisting_early_debt() {
    let store = InMemorySandboxStore::default();
    let mut current = sandbox();
    current.revision = 1;
    current.runtime.as_mut().unwrap().provision_generation = Some(1);
    current.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    let mut writer = agent("agt_quiet", "conversation-quiet", 1);
    writer.revision = 1;
    store.create_agent(writer.clone()).await.unwrap();
    let event = store
        .append_agent_log(
            &writer.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        )
        .await
        .unwrap();
    for at in [NOW, BEFORE, BOUNDARY] {
        let mut next = current.clone();
        next.begin_checkpoint_capture(at.into()).unwrap();
        let request = SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&current),
            sandbox: next.clone(),
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: at.into(),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        };
        if at == BOUNDARY {
            store.commit_sandbox_lifecycle(request).await.unwrap();
            assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(next));
        } else {
            assert_eq!(
                store.commit_sandbox_lifecycle(request).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(
                store.get_sandbox(&current.id).await.unwrap(),
                Some(current.clone())
            );
            assert!(store
                .sandbox_events(&current.id, 0, 10)
                .await
                .unwrap()
                .is_empty());
        }
        assert_eq!(
            store.agent_log(&writer.id, 0, 10).await.unwrap(),
            vec![event.clone()]
        );
        assert_eq!(
            store.get_agent(&writer.id).await.unwrap(),
            Some(writer.clone())
        );
    }
}
