use super::*;
use layer2_core::{Authorization, Caller, Capability};

pub(super) fn inject(
    h: &mut Harness,
    skills: Arc<dyn AgentSkillResolver>,
    fault: Fault,
) -> Arc<PublicationStore> {
    inject_with_ownership(h, skills, fault, None)
}

pub(super) fn inject_with_ownership(
    h: &mut Harness,
    skills: Arc<dyn AgentSkillResolver>,
    fault: Fault,
    ownership: Option<(
        Arc<layer2_core::agent_ownership::OwnershipCoordinator>,
        Arc<layer2_core::agent_ownership::test_support::MemoryOwnershipStore>,
    )>,
) -> Arc<PublicationStore> {
    let store = Arc::new(PublicationStore {
        inner: h.store.clone(),
        attempts: AtomicUsize::new(0),
        fault,
        ownership: ownership.as_ref().map(|(_, store)| store.clone()),
    });
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.ownership = ownership.map(|(coordinator, _)| coordinator);
    config.lease_duration = Duration::from_secs(30);
    config.retry_delay = Duration::from_secs(1);
    config.open_timeout = Duration::from_secs(2);
    h.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills,
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        config,
    );
    store
}

pub(super) async fn claim(h: &Harness, id: &AgentId) -> Agent {
    h.store
        .claim_agent_work(
            id,
            "agent-worker",
            rfc3339_at(h.now),
            rfc3339_at(h.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn calls(h: &Harness, operation: WireOperation) -> usize {
    h.backend
        .requests()
        .await
        .iter()
        .filter(|(_, request)| request.operation() == operation)
        .count()
}

pub(super) async fn skill_fixture() -> (Harness, Arc<ScriptedSkills>, Agent) {
    let material = super::super::skill_material_tests::runtime_skill("7.0", 'a', "archive");
    let skills = ScriptedSkills::new([Ok(vec![]), Ok(vec![material])]);
    let mut h = Harness::new(skills.clone()).await;
    h.now = SystemTime::now();
    let admitted = h.admit("publication-skill", vec![]).await;
    let ready = h.drive_ready(&admitted.id).await;
    AgentOperationService::new(h.store.clone(), h.registry.clone())
        .install_skill(
            &Caller::new(
                ready.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            &ready.id,
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                version: Some("7.0".into()),
            },
        )
        .await
        .unwrap();
    h.now = SystemTime::now();
    let claimed = claim(&h, &ready.id).await;
    (h, skills, claimed)
}
