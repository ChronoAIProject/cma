use super::*;
use layer2_core::sandbox::{
    RetainedRecoveryDisposition as D, RetainedRecoveryInspector, RetainedRecoverySnapshot,
};

#[tokio::test]
async fn retained_recovery_assessment_reads_exact_native_history_without_effects() {
    let fixture = Fixture::native().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(adapter.as_ref(), &active, "test")
        .await
        .unwrap();
    let binding = active.runtime.as_ref().unwrap();
    let status = adapter
        .runtime_status(&RuntimeHandle::new(&binding.runtime_id))
        .await
        .unwrap();
    let mut failed = active.clone();
    failed.status = SandboxStatus::Failed;
    failed.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "fixture".into(),
    });
    failed
        .work
        .schedule(SandboxWorkStep::None, failed.updated_at.clone());
    let mut child = lifecycle_queue_tests::ready_agent(&active, now);
    child.status = AgentStatus::Paused;
    child.conversation.runtime_generation = binding.provision_generation;
    child.conversation.conversation_id = Some(uuid::Uuid::new_v4().to_string());
    let original = RetainedRecoverySnapshot {
        sandbox: failed,
        agents: vec![child.clone()],
        workspaces: vec![],
    };
    let inspector =
        crate::sandbox::retained_recovery::RetainedRuntimeInspector::new(adapter, "test".into());
    let calls_before = fixture.backend.calls.snapshot().len();
    for (case, expected) in [
        ("same_boot", D::RecordedBootPresent),
        ("replaced_boot", D::ExactBootReplacement),
        ("healthy_replacement_work", D::ChildActivity),
        ("missing_history", D::ProviderEvidenceRefused),
        ("invalid_history", D::ProviderEvidenceRefused),
        ("plan_mismatch", D::ProviderEvidenceRefused),
        ("digest_mismatch", D::ProviderEvidenceRefused),
        ("unavailable", D::EvidenceUnavailable),
        ("foreign_home", D::AuthorityChanged),
        ("restore_pending", D::EvidenceUnavailable),
        ("changed_after_history", D::AuthorityChanged),
    ] {
        let mut current = status.clone();
        current.agent_processes = 0;
        *fixture.backend.recovery_code.lock().unwrap() = None;
        match case {
            "replaced_boot" => current.boot_id = "a".repeat(32),
            "healthy_replacement_work" => {
                current.boot_id = "a".repeat(32);
                current.agent_processes = 1;
            }
            "foreign_home" => current.home_identity = "z".repeat(43),
            "restore_pending" => {
                current.restore_pending = true;
                current.installation_ready = false;
            }
            "missing_history" => {
                *fixture.backend.recovery_code.lock().unwrap() = Some("history_missing")
            }
            "invalid_history" => {
                *fixture.backend.recovery_code.lock().unwrap() = Some("history_invalid")
            }
            "plan_mismatch" => {
                *fixture.backend.recovery_code.lock().unwrap() = Some("plan_mismatch")
            }
            "digest_mismatch" => {
                *fixture.backend.recovery_code.lock().unwrap() = Some("digest_mismatch")
            }
            "unavailable" => *fixture.backend.recovery_code.lock().unwrap() = Some("unavailable"),
            "changed_after_history" => {
                fixture
                    .backend
                    .replace_after_history
                    .store(true, Ordering::SeqCst);
            }
            _ => {}
        }
        *fixture.backend.recovery_plan.lock().unwrap() = serde_json::json!({
            "runtime": { "sandboxId": active.id.as_str(), "generation": binding.provision_generation.unwrap(),
                "homeIdentity": original.source().unwrap().home_identity, "bootId": current.boot_id },
            "agents": [{"agentId": child.id.as_str(), "conversationId": child.conversation.conversation_id.clone().unwrap()}],
            "histories": [{"agentId": child.id.as_str(), "conversationId": child.conversation.conversation_id.clone().unwrap(), "historySha256": "b".repeat(64)}]
        });
        *fixture.backend.status_override.lock().unwrap() =
            (case != "changed_after_history").then_some(current);
        let result = inspector.inspect(&original).await;
        assert_eq!(result, expected, "{case}");
        assert_eq!(original.sandbox.runtime.as_ref(), Some(binding));
        assert_eq!(fixture.harness.stored(&active.id).await, active);
    }
    assert!(!fixture
        .backend
        .calls
        .snapshot()
        .iter()
        .skip(calls_before)
        .any(|call| matches!(
            call.as_str(),
            "suspend" | "resume" | "upgrade" | "delete-retained" | "create"
        )));
}

#[tokio::test]
async fn retained_recovery_absence_requires_exact_suspension_and_no_child_history() {
    let fixture = Fixture::native().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.pause(&active, &mut now).await;
    let mut failed = active.clone();
    failed.status = SandboxStatus::Failed;
    failed.failure = Some(TypedFailure {
        class: "runtime_lost".into(),
        detail: "fixture".into(),
    });
    failed
        .work
        .schedule(SandboxWorkStep::None, failed.updated_at.clone());
    let mut snapshot = RetainedRecoverySnapshot {
        sandbox: failed,
        agents: vec![],
        workspaces: vec![],
    };
    let inspector = crate::sandbox::retained_recovery::RetainedRuntimeInspector::new(
        fixture.adapter(&runtime_image()),
        "test".into(),
    );
    let before = fixture.backend.calls.snapshot().len();
    assert_eq!(inspector.inspect(&snapshot).await, D::ExactSuspendedRuntime);
    let mut child = lifecycle_queue_tests::ready_agent(&active, now);
    child.status = AgentStatus::Paused;
    snapshot.agents.push(child);
    assert_ne!(inspector.inspect(&snapshot).await, D::ExactSuspendedRuntime);
    snapshot.agents.clear();
    fixture.backend.lose_object().await;
    assert_ne!(
        inspector.inspect(&snapshot).await,
        D::ExactSuspendedRuntime,
        "missing provider object is not a verified retained suspension"
    );
    assert!(!fixture
        .backend
        .calls
        .snapshot()
        .iter()
        .skip(before)
        .any(|call| matches!(
            call.as_str(),
            "suspend" | "resume" | "upgrade" | "delete-retained" | "create"
        )));
}
