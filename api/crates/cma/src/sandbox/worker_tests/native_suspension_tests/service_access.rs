use super::*;
use layer2_core::sandbox::store::{SandboxTransaction, SandboxTransactionStore};

#[tokio::test]
async fn native_scope_grants_resume_exact_old_identity_with_new_scope_after_each_restart() {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let mut active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    let initial = fixture.backend.snapshot().await;
    let original_request = fixture.backend.request().await;
    for service in ["api-github", "api-linear"] {
        let previous = active.clone();
        let paused = fixture.pause(&active, &mut now).await;
        let mut granted = paused.clone();
        let grant = granted
            .grant_service_access(&active.owner, &[ServiceId::new(service)], rfc3339_at(now))
            .unwrap()
            .unwrap();
        let generation_floor = grant.credential_generation_floor;
        fixture
            .harness
            .store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: paused.revision,
                sandbox: Box::new(granted.clone()),
                event_at: rfc3339_at(now),
                event: SandboxEventKind::ServiceAccessGranted { grant },
            })
            .await
            .unwrap();
        // The old request is recoverable from existing serialized custody and
        // grant floors; it does not depend on a new schema field or process cache.
        let recovered: Sandbox =
            serde_json::from_slice(&serde_json::to_vec(&granted).unwrap()).unwrap();
        let binding = recovered.lifecycle.suspended_runtime.as_deref().unwrap();
        let old_request = crate::sandbox::runtime_attachment::retained_runtime_request(
            &recovered, binding, "test",
        )
        .unwrap();
        assert_eq!(
            old_request.spec.reaches,
            previous
                .credential
                .effective_reaches()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert!(!old_request.spec.reaches.contains(&service.to_owned()));
        let mut missing_custody = recovered.clone();
        missing_custody.credential.current = None;
        missing_custody.credential.delivered = false;
        assert!(
            crate::sandbox::runtime_attachment::retained_runtime_request(
                &missing_custody,
                binding,
                "test"
            )
            .is_err()
        );
        let mut foreign_binding = binding.clone();
        foreign_binding.runtime_id = "agent-sandbox:another:foreign-uid".into();
        assert!(
            crate::sandbox::runtime_attachment::retained_runtime_request(
                &recovered,
                &foreign_binding,
                "test"
            )
            .is_err()
        );
        fixture.backend.lose_resume.store(true, Ordering::SeqCst);
        fixture
            .harness
            .store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                granted.id.clone(),
                granted.owner.clone(),
                rfc3339_at(now),
                SandboxReviveCause::ExplicitRequest,
            ))
            .await
            .unwrap();
        active = fixture
            .drive(
                &granted.id,
                &mut now,
                SandboxStatus::Active,
                "cma-runtime:changed-default",
            )
            .await;
        let old = previous.runtime.as_ref().unwrap();
        let new = active.runtime.as_ref().unwrap();
        assert_eq!(new.runtime_id, old.runtime_id);
        assert_eq!(new.runtime_version, old.runtime_version);
        assert!(new.provision_generation > old.provision_generation);
        assert_ne!(new.boot_id, old.boot_id);
        assert_eq!(active.lifecycle.home, previous.lifecycle.home);
        assert_eq!(active.profile, previous.profile);
        assert!(active.lifecycle.suspended_runtime.is_none());
        assert!(active.credential.current.as_ref().unwrap().generation > generation_floor);
        let resumed_request = fixture.backend.request().await;
        assert_eq!(resumed_request.image, original_request.image);
        assert_eq!(resumed_request.volumes, original_request.volumes);
        assert_eq!(
            resumed_request.env["CHRONO_SANDBOX_REACHES"],
            active
                .credential
                .effective_reaches()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        let restored = fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.status, AgentStatus::Ready);
        assert_eq!(
            restored.conversation.conversation_id,
            agent.conversation.conversation_id
        );
        assert_eq!(restored.worktree_id, agent.worktree_id);
        assert!(restored.conversation.runtime_generation > agent.conversation.runtime_generation);
        let snapshot = fixture.backend.snapshot().await;
        assert_eq!(snapshot.workspaces, initial.workspaces);
        assert_eq!(snapshot.worktrees, initial.worktrees);
        assert_eq!(snapshot.homes, initial.homes);
    }
    let calls = fixture.backend.calls.snapshot();
    assert_eq!(calls.iter().filter(|call| *call == "create").count(), 1);
    assert_eq!(calls.iter().filter(|call| *call == "resume").count(), 2);
    assert_eq!(
        calls.iter().filter(|call| *call == "resume-replay").count(),
        2
    );
    assert!(!calls.iter().any(|call| call == "delete-retained"));
}

async fn revise_hot(
    fixture: &Fixture,
    active: &Sandbox,
    names: &[&str],
    now: &mut SystemTime,
) -> Sandbox {
    let mut changed = active.clone();
    let scope = changed
        .revise_live_service_access(
            &active.owner,
            &names
                .iter()
                .map(|name| ServiceId::new(*name))
                .collect::<Vec<_>>(),
            rfc3339_at(*now),
        )
        .unwrap();
    let floor = scope.credential_generation_floor;
    fixture
        .harness
        .store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: active.revision,
            sandbox: Box::new(changed),
            event_at: rfc3339_at(*now),
            event: SandboxEventKind::ServiceAccessRevised {
                scope: Box::new(scope),
            },
        })
        .await
        .unwrap();
    for _ in 0..10 {
        fixture.step(&active.id, *now, &runtime_image()).await;
        let next = fixture.harness.stored(&active.id).await;
        assert_eq!(next.runtime, active.runtime);
        assert_eq!(next.lifecycle, active.lifecycle);
        assert!(next.failure.is_none(), "{:?}", next.failure);
        if next.status == SandboxStatus::Active
            && next.credential.current.as_ref().unwrap().generation > floor
            && next.credential.revoke_debt.is_empty()
        {
            return next;
        }
        *now += Duration::from_secs(1);
    }
    panic!("hot scope did not settle");
}

#[tokio::test]
async fn hot_scope_keeps_original_native_request_until_explicit_resume_then_pins_new_request() {
    let fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let original = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let startup_request = fixture.backend.request().await;
    let first = revise_hot(&fixture, &original, &["baseline", "api-github"], &mut now).await;
    let second = revise_hot(&fixture, &first, &["baseline", "calendar"], &mut now).await;
    assert_eq!(fixture.backend.request().await, startup_request);
    let paused = fixture.pause(&second, &mut now).await;
    let retained = crate::sandbox::runtime_attachment::retained_runtime_request(
        &paused,
        paused.lifecycle.suspended_runtime.as_deref().unwrap(),
        "test",
    )
    .unwrap();
    assert_eq!(
        retained.spec.reaches.join(","),
        startup_request.env["CHRONO_SANDBOX_REACHES"]
    );
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
    let resumed = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    assert_eq!(
        resumed.runtime.as_ref().unwrap().runtime_id,
        original.runtime.as_ref().unwrap().runtime_id
    );
    assert!(
        resumed.runtime.as_ref().unwrap().provision_generation
            > original.runtime.as_ref().unwrap().provision_generation
    );
    assert_eq!(resumed.lifecycle.home, original.lifecycle.home);
    let resumed_request = fixture.backend.request().await;
    assert_eq!(
        resumed_request.env["CHRONO_SANDBOX_REACHES"],
        "baseline,calendar"
    );
    let third = revise_hot(&fixture, &resumed, &["baseline"], &mut now).await;
    assert_eq!(fixture.backend.request().await, resumed_request);
    assert_eq!(
        third.credential.effective_reaches(),
        vec![ServiceId::new("baseline")]
    );
    assert_eq!(
        third
            .credential
            .service_scope
            .as_ref()
            .unwrap()
            .runtime_services,
        vec![ServiceId::new("baseline"), ServiceId::new("calendar")]
    );
}
