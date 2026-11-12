use super::*;

#[tokio::test]
async fn unavailable_upgrade_resolution_settles_after_five_attempts_and_accepts_input() {
    let skills = ScriptedSkills::new(
        std::iter::once(Ok(vec![runtime_skill("1.0", 'a', "old")])).chain((0..5).map(|_| {
            Err(AgentSkillResolutionError::Unavailable(
                "catalog unavailable".into(),
            ))
        })),
    );
    let harness = Harness::with_upgrade_support(skills.clone()).await;
    let admitted = harness
        .admit("bounded-upgrade", vec![reference("1.0")])
        .await;
    let ready = harness.drive_ready(&admitted.id).await;
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
        .with_skill_upgrades_enabled(true);
    operations
        .install_skill(&caller, &ready.id, reference("1.1"))
        .await
        .unwrap();
    let now = SystemTime::now();
    for attempt in 0..5 {
        // Recreate the worker each time: the budget lives in retained work state.
        restarted(&harness, skills.clone())
            .run_one(&ready.id, now + Duration::from_secs(2 * attempt))
            .await
            .unwrap();
        let state = harness.stored(&ready.id).await;
        if attempt == 1 {
            assert_eq!(state.work.live_skills[0].preparation_retries, 2);
            // Connection publication resets phase-level attempts but retains this intent.
            let mut resumed = state.clone();
            resumed.work.attempts = 0;
            resumed.revision += 1;
            harness
                .store
                .save_agent(state.revision, resumed)
                .await
                .unwrap();
        }
        assert_eq!(
            state.work.live_skills[0].status,
            if attempt < 4 {
                AgentSkillInstallStatus::Pending
            } else {
                AgentSkillInstallStatus::Failed
            }
        );
    }
    let failed = harness.stored(&ready.id).await;
    assert!(layer2_core::sandbox::agent_live_skills::idle(&failed));
    assert!(failed.work.live_skills[0]
        .error
        .as_deref()
        .unwrap()
        .starts_with("agent_skill_upgrade_preparation_exhausted:"));
    assert_eq!(
        effective(&failed.work, &reference("").id).unwrap().version,
        "1.0"
    );
    assert_eq!(skills.calls.load(Ordering::Relaxed), 6);
    assert!(matches!(
        operations
            .send_message(
                &caller,
                &ready.id,
                "Continue after preparation failure.".into()
            )
            .await
            .unwrap(),
        layer2_core::sandbox::AgentMessageOutcome::Dispatched(_)
    ));
}

#[tokio::test]
async fn confirmed_upgrade_after_a_lost_reply_becomes_idle_and_accepts_input() {
    let versions = Versions::new(vec![
        vec![runtime_skill("1.0", 'a', "old")],
        vec![runtime_skill("1.1", 'b', "new")],
    ]);
    let harness = Harness::with_upgrade_support(versions).await;
    let admitted = harness
        .admit("confirmed-upgrade", vec![reference("1.0")])
        .await;
    let ready = harness.drive_ready(&admitted.id).await;
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
        .with_skill_upgrades_enabled(true);
    operations
        .install_skill(&caller, &ready.id, reference("1.1"))
        .await
        .unwrap();
    harness
        .backend
        .lose_next_response(WireOperation::InstallAgentSkill);
    harness.run(&ready.id, SystemTime::now()).await;
    assert_eq!(
        harness.stored(&ready.id).await.work.live_skills[0].status,
        AgentSkillInstallStatus::Pending
    );
    restarted(&harness, Versions::new(vec![]))
        .run_one(&ready.id, SystemTime::now() + Duration::from_secs(2))
        .await
        .unwrap();
    let installed = harness.stored(&ready.id).await;
    assert!(layer2_core::sandbox::agent_live_skills::idle(&installed));
    assert_eq!(
        effective(&installed.work, &reference("").id)
            .unwrap()
            .version,
        "1.1"
    );
    assert!(matches!(
        operations
            .send_message(&caller, &ready.id, "Continue with the new version.".into())
            .await
            .unwrap(),
        layer2_core::sandbox::AgentMessageOutcome::Dispatched(_)
    ));
}
