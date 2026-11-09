use super::*;
use layer2_core::sandbox::{
    SandboxUnpublishedRecoverySnapshot, SandboxUnpublishedRecoveryVerifier, TypedFailure,
};

#[path = "evidence_refusal_cases.rs"]
mod refusal_cases;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
async fn original_evidence_keeps_typed_refusals_and_never_weakens_boot_or_home_proof() {
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
    let paused = fixture.pause(&active, &mut now).await;
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    let active = fixture
        .drive(
            &paused.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let initial_calls = fixture.backend.calls.snapshot();
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
    child.conversation.runtime_generation = Some(1);
    child.conversation.conversation_id = Some(uuid::Uuid::new_v4().to_string());
    let snapshot = SandboxUnpublishedRecoverySnapshot {
        sandbox: failed,
        agents: vec![child.clone()],
        workspaces: vec![],
    };
    snapshot.validate().unwrap();
    *fixture.backend.recovery_plan.lock().unwrap() = serde_json::json!({
        "runtime": { "sandboxId": active.id.as_str(), "generation": binding.provision_generation.unwrap(), "homeIdentity": snapshot.home_identity().unwrap(), "bootId": binding.boot_id.clone().unwrap() },
        "agents": [{"agentId": child.id.as_str(), "conversationId": child.conversation.conversation_id.clone().unwrap()}],
        "histories": [{"agentId": child.id.as_str(), "conversationId": child.conversation.conversation_id.clone().unwrap(), "historySha256": "b".repeat(64)}]
    });
    let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
        adapter,
        "test".into(),
    );
    for (case, reason, provider_response) in [
        ("valid", "Success"),
        ("unavailable", "Unavailable"),
        ("boot", "BootMismatch"),
        ("home", "HomeChanged"),
        ("restore", "RestorePending"),
        ("invalid_restore", "NativeChanged"),
        ("history_missing", "MissingHistory"),
        ("history_invalid", "CorruptHistory"),
        ("history_incomplete", "CorruptHistory"),
        ("plan_missing", "CorruptHistory"),
        ("plan_mismatch", "CorruptHistory"),
        ("digest_mismatch", "CorruptHistory"),
        ("history_set", "CorruptHistory"),
        ("deadline", "Deadline"),
    ]
    .into_iter()
    .map(|(case, reason)| (case, reason, None))
    .chain(refusal_cases::provider_refusal_cases())
    .chain([("replaced_after", "BootMismatch", None)])
    {
        let mut current = status.clone();
        *fixture.backend.recovery_code.lock().unwrap() = None;
        *fixture.backend.recovery_response.lock().unwrap() = provider_response
            .as_ref()
            .map(|response| serde_json::to_vec(response).unwrap());
        let history_calls = || {
            fixture
                .backend
                .calls
                .snapshot()
                .iter()
                .filter(|call| call.as_str() == "inspect-history")
                .count()
        };
        let history_before = history_calls();
        match case {
            "boot" => current.boot_id = "a".repeat(32),
            "home" => {}
            "restore" => {
                current.restore_pending = true;
                current.installation_ready = false;
                current.agent_processes = 0;
                current
                    .validate_for(&layer3_adapter::RuntimeStatusRequest {
                        sandbox_id: current.sandbox_id.clone(),
                        generation: current.generation,
                        home_identity: current.home_identity.clone(),
                    })
                    .unwrap();
            }
            // Contradictory status is rejected before it can be restore proof.
            "invalid_restore" => current.restore_pending = true,
            "valid" => {}
            "replaced_after" => {
                fixture
                    .backend
                    .replace_after_history
                    .store(true, Ordering::SeqCst);
            }
            _ if provider_response.is_some() => {}
            _ => *fixture.backend.recovery_code.lock().unwrap() = Some(case),
        }
        *fixture.backend.status_override.lock().unwrap() =
            (case != "replaced_after").then_some(current);
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let span = tracing::info_span!("private_caller", secret = "private caller content");
        let _entered = span.enter();
        let result = if case == "home" {
            let source = layer3_adapter::RuntimeRecoveryInspectionRequest {
                runtime: RuntimeHandle::new(&binding.runtime_id),
                boot_id: binding.boot_id.clone().unwrap(),
                conversations: vec![],
                identity: layer3_adapter::RuntimeStatusRequest {
                    sandbox_id: active.id.to_string(),
                    generation: binding.provision_generation.unwrap(),
                    home_identity: "z".repeat(43),
                },
            };
            crate::sandbox::unpublished_recovery::verify_source(
                fixture.adapter(&runtime_image()).as_ref(),
                &snapshot.sandbox,
                &source,
                "test",
            )
            .await
            .map_err(|error| {
                layer2_core::sandbox::SandboxStoreError::Unavailable(error.to_string())
            })
        } else {
            verifier.verify(&snapshot).await.map(|proof| {
                proof.validate_for(&snapshot).unwrap();
                assert_eq!(proof.conversations.len(), 1);
                assert_eq!(proof.conversations[0].agent_id, child.id);
                assert_eq!(
                    proof.conversations[0].conversation_id,
                    child.conversation.conversation_id.as_deref().unwrap()
                );
            })
        };
        assert_eq!(result.is_ok(), case == "valid", "{case}: {result:?}");
        if case == "unavailable" {
            assert!(result
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("private provider detail"));
        }
        if provider_response.is_some() {
            assert_eq!(history_calls(), history_before + 1, "{case}");
            assert!(result
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("original runtime recovery evidence is incomplete or changed"));
        }
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let diagnostics = logs
            .lines()
            .filter(|line| line.contains("unpublished recovery stage"))
            .collect::<Vec<_>>();
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains(&format!("reason={reason}"))),
            "{case}: {logs}"
        );
        if provider_response.is_some() {
            assert!(
                diagnostics.iter().any(|line| line.contains("stage=History")
                    && line.contains(&format!("reason={reason}"))),
                "{case}: {logs}"
            );
        }
        for line in diagnostics {
            assert!(
                line.contains("elapsed_ms=")
                    && line.contains("attempted=")
                    && line.contains("admitted=")
            );
            for secret in [
                active.id.as_str(),
                binding.runtime_id.as_str(),
                "private caller content",
                "private provider detail",
            ] {
                assert!(!line.contains(secret), "{line}");
            }
            if let Some(code) = provider_response.as_ref().and_then(|r| r["code"].as_str()) {
                assert!(!line.contains(code), "raw provider code leaked: {line}");
            }
        }
        assert_eq!(fixture.harness.stored(&active.id).await, active);
        assert!(!fixture
            .backend
            .calls
            .snapshot()
            .iter()
            .skip(initial_calls.len())
            .any(|call| call == "suspend" || call == "delete-retained"));
    }
}
