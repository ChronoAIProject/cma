use super::*;
use layer2_core::{sandbox::AgentWorkStep, SandboxId};

fn candidate(index: usize) -> AgentConnectionRecoveryCandidate {
    AgentConnectionRecoveryCandidate {
        agent_id: AgentId(format!("agt_maintenance_{index}")),
        sandbox_id: SandboxId("sbx_maintenance".into()),
        conversation_id: "conversation".into(),
        runtime_generation: 1,
        revision: 1,
        work_step: AgentWorkStep::None,
        managed_guidance_revision: 0,
        managed_context_eligible: true,
    }
}

#[tokio::test(start_paused = true)]
async fn unchanged_maintenance_retries_boundedly_and_changed_authority_is_immediate() {
    let checks = MaintenanceChecks::default();
    let mut candidate = candidate(0);
    assert!(checks.should_check(&candidate, 1));
    assert!(!checks.should_check(&candidate, 1));
    tokio::time::advance(RETRY_INTERVAL - Duration::from_millis(1)).await;
    assert!(!checks.should_check(&candidate, 1));
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(checks.should_check(&candidate, 1));
    candidate.revision += 1;
    assert!(checks.should_check(&candidate, 1));
    candidate.runtime_generation += 1;
    assert!(checks.should_check(&candidate, 1));
    candidate.conversation_id = "replacement".into();
    assert!(checks.should_check(&candidate, 1));
    assert!(checks.should_check(&candidate, 2));
    assert!(!checks.should_check(&candidate, 2));
    // Restart loses only an optimization; it cannot suppress recovery.
    assert!(MaintenanceChecks::default().should_check(&candidate, 2));
}

#[tokio::test(start_paused = true)]
async fn production_shaped_idle_sweeps_reduce_maintenance_admissions() {
    let checks = MaintenanceChecks::default();
    // Production summaries show about 15 eligible maintenance checks per sweep.
    let candidates: Vec<_> = (0..15).map(candidate).collect();
    let mut before = 0;
    let mut after = 0;
    for _ in 0..300 {
        for candidate in &candidates {
            before += 1;
            after += usize::from(checks.should_check(candidate, 1));
        }
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    assert_eq!((before, after), (4500, 150));
    println!("maintenance admissions in 300 one-second sweeps: {before} -> {after}");
}

#[tokio::test(start_paused = true)]
async fn bounded_tracking_never_blocks_untracked_agents() {
    let checks = MaintenanceChecks::default();
    for index in 0..MAX_TRACKED + 1 {
        assert!(checks.should_check(&candidate(index), 1));
    }
    assert_eq!(checks.0.lock().unwrap().len(), MAX_TRACKED);
    assert!(checks.should_check(&candidate(MAX_TRACKED), 1));
    tokio::time::advance(RETRY_INTERVAL).await;
    assert!(checks.should_check(&candidate(MAX_TRACKED), 1));
    assert_eq!(checks.0.lock().unwrap().len(), 1);
}
