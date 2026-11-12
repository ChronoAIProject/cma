use super::*;

#[tokio::test]
async fn transient_skill_resolution_retries_then_materializes_exact_archive() {
    let reference = SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        version: Some("7.0".into()),
    };
    let material = AgentSkill {
        id: "research".into(),
        name: None,
        version: Some("7.0".into()),
        archive_sha256: "e".repeat(64),
        source_url: "https://ornn.example/skills/research/7.0.tar.gz".into(),
    };
    let skills = ScriptedSkills::new([
        Err(AgentSkillResolutionError::Unavailable(
            "temporary outage".into(),
        )),
        Ok(vec![material.clone()]),
    ]);
    let harness = Harness::new(skills.clone()).await;
    let agent = harness.admit("skills", vec![reference]).await;

    harness.run(&agent.id, harness.now).await;
    harness.run(&agent.id, harness.now).await;
    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.status, AgentStatus::Starting);
    assert_eq!(retrying.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(retrying.work.attempts, 1);
    assert!(matches!(
        harness
            .store
            .agent_log(&agent.id, 0, 20)
            .await
            .unwrap()
            .last()
            .map(|entry| &entry.kind),
        Some(AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::MaterializeHome,
            failure,
        }) if failure.class == "agent_skill_source_unavailable"
    ));

    for _ in 0..3 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(2))
            .await;
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    let home = &snapshot.homes[&AgentSlot::new(agent.id.to_string())];
    assert_eq!(home.skills, [material]);
    assert_eq!(skills.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        ready.work.resolved_skills.as_ref().unwrap()[0].version,
        "7.0"
    );
}

#[tokio::test]
async fn lost_home_reply_retries_with_a_rotated_locator_for_the_same_archive() {
    let reference = SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        version: Some("@stable".into()),
    };
    let first = AgentSkill {
        id: "research".into(),
        name: None,
        version: Some("7.0".into()),
        archive_sha256: "e".repeat(64),
        source_url: "https://ornn.example/temporary/a.tar.gz".into(),
    };
    let second = AgentSkill {
        source_url: "https://ornn.example/temporary/b.tar.gz".into(),
        ..first.clone()
    };
    let skills = ScriptedSkills::new([Ok(vec![first.clone()]), Ok(vec![second])]);
    let harness = Harness::new(skills.clone()).await;
    let agent = harness
        .admit("rotated-skill-locator", vec![reference])
        .await;

    harness.run(&agent.id, harness.now).await;
    harness
        .backend
        .lose_next_response(WireOperation::MaterializeAgentHome);
    harness.run(&agent.id, harness.now).await;
    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(retrying.work.attempts, 1);
    assert_eq!(retrying.work.cleanup_debt.len(), 1);

    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(2))
            .await;
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert!(ready.work.cleanup_debt.is_empty());
    assert_eq!(skills.calls.load(Ordering::Relaxed), 1);
    assert_eq!(ready.profile, agent.profile);
    assert_eq!(retrying.work.resolved_skills, ready.work.resolved_skills);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(
        snapshot.homes[&AgentSlot::new(agent.id.to_string())].skills,
        [first]
    );
}
