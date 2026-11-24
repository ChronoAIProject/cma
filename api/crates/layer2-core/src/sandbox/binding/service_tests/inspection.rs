use super::*;
use crate::sandbox::{
    AgentInstructionPolicy, AgentSkillInstallStatus, AgentSkillInstallation, ResolvedAgentSkill,
};

struct FixtureInstructions;

impl AgentInstructionPolicy for FixtureInstructions {
    fn owner_prompt(&self, agent: &Agent) -> String {
        agent
            .system_prompt()
            .strip_prefix("Managed\n\n")
            .unwrap()
            .into()
    }

    fn compose(&self, _agent: &Agent, prompt: &str) -> String {
        format!("Managed\n\n{prompt}")
    }
}

#[tokio::test]
async fn inspection_reports_confirmed_prompt_and_skill_drift() {
    let fixture = fixture();
    let mut requested = profile("Original instruction");
    requested["context"]["skills"] = json!([{"source":"ornn","id":"example","version":"1"}]);
    let binding = fixture.ready_with_profile(requested, None).await;
    let conversation = resource_access::ready_conversation(&fixture, &binding).await;
    let mut agent = fixture
        .store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let reference = binding.profile.agent.skills[0].clone();
    let original = ResolvedAgentSkill {
        source: reference.source,
        id: reference.id.clone(),
        name: None,
        version: "1".into(),
        archive_sha256: "a".repeat(64),
    };
    agent.work.resolved_skills = Some(vec![original.clone()]);
    assert_eq!(
        BindingProfileDrift::compare(&binding, Some(&agent), &FixtureInstructions),
        BindingProfileDrift {
            system_prompt: DriftState::Unknown,
            skills: DriftState::Unknown,
        }
    );

    agent.ready_at = Some(crate::now_rfc3339());
    let mut settings = agent.settings();
    settings.system_prompt =
        Some(FixtureInstructions.compose(&agent, &binding.profile.agent.system_prompt));
    agent.conversation.settings = Some(settings);
    assert_eq!(
        BindingProfileDrift::compare(&binding, Some(&agent), &FixtureInstructions),
        BindingProfileDrift {
            system_prompt: DriftState::Match,
            skills: DriftState::Match
        }
    );

    agent.conversation.settings.as_mut().unwrap().system_prompt = Some("Managed\n\nEdited".into());
    let mut upgrade = AgentSkillInstallation::new(crate::profiles::SkillRef {
        version: Some("2".into()),
        ..reference
    });
    upgrade.previous = Some(original.clone());
    agent.work.live_skills.push(upgrade.clone());
    assert_eq!(
        BindingProfileDrift::compare(&binding, Some(&agent), &FixtureInstructions),
        BindingProfileDrift {
            system_prompt: DriftState::Drift,
            skills: DriftState::Match
        }
    );

    upgrade.status = AgentSkillInstallStatus::Installed;
    upgrade.material = Some(ResolvedAgentSkill {
        version: "2".into(),
        ..original
    });
    *agent.work.live_skills.last_mut().unwrap() = upgrade;
    assert_eq!(
        BindingProfileDrift::compare(&binding, Some(&agent), &FixtureInstructions),
        BindingProfileDrift {
            system_prompt: DriftState::Drift,
            skills: DriftState::Drift
        }
    );
    let foreign = Caller::new(
        SubjectId::new("foreign"),
        fixture.caller.authorization.clone(),
    );
    assert!(matches!(
        fixture
            .service
            .inspect_conversation_with_profile_drift(
                &foreign,
                &binding.binding_id,
                &conversation.conversation_id,
                Some(&FixtureInstructions),
            )
            .await,
        Err(BindingError::NotFound)
    ));
}

#[tokio::test]
async fn inspection_reports_the_retained_mapping_and_agent_lifecycle_without_rebinding() {
    let fixture = fixture();
    let binding = fixture.ready().await;
    let conversation = resource_access::ready_conversation(&fixture, &binding).await;
    let mut agent = fixture
        .store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let inspected = fixture
        .service
        .inspect_conversation(
            &fixture.caller,
            &binding.binding_id,
            &conversation.conversation_id,
        )
        .await
        .unwrap();
    assert_eq!(inspected.conversation, conversation);
    assert_eq!(inspected.agent_status, Some(agent.status));
    let revision = agent.revision;
    agent
        .transition(
            AgentStatus::Stopped,
            crate::now_rfc3339(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    fixture
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    let revision = agent.revision;
    agent
        .transition(AgentStatus::Deleted, crate::now_rfc3339(), None)
        .unwrap();
    fixture.store.save_agent(revision, agent).await.unwrap();
    let inspected = fixture
        .service
        .inspect_conversation(
            &fixture.caller,
            &binding.binding_id,
            &conversation.conversation_id,
        )
        .await
        .unwrap();
    assert_eq!(inspected.conversation, conversation);
    assert_eq!(inspected.agent_status, Some(AgentStatus::Deleted));
    assert_eq!(fixture.binding().await, binding);
}

#[tokio::test]
async fn inspection_requires_the_binding_manager_and_never_reserves_a_missing_mapping() {
    let fixture = fixture();
    let binding = fixture.ready().await;
    assert!(matches!(
        fixture
            .service
            .inspect_conversation(&fixture.caller, &binding.binding_id, "unknown_conversation")
            .await,
        Err(BindingError::NotFound)
    ));
    assert!(fixture
        .store
        .binding_conversations(&binding.binding_id, binding.generation, None, 20)
        .await
        .unwrap()
        .is_empty());
    let (conversation, _) = fixture
        .service
        .ensure_conversation(
            &fixture.caller,
            &binding.binding_id,
            "reserved_conversation",
        )
        .await
        .unwrap();
    let inspected = fixture
        .service
        .inspect_conversation(
            &fixture.caller,
            &binding.binding_id,
            &conversation.conversation_id,
        )
        .await
        .unwrap();
    assert!(inspected.agent_status.is_none());
    assert_eq!(inspected.conversation, conversation);
    let stranger = Caller::new(
        SubjectId::new("other-manager"),
        fixture.caller.authorization.clone(),
    );
    assert!(matches!(
        fixture
            .service
            .inspect_conversation(
                &stranger,
                &binding.binding_id,
                &conversation.conversation_id
            )
            .await,
        Err(BindingError::NotFound)
    ));
    let forbidden = Caller::new(fixture.caller.subject.clone(), Authorization::none());
    assert!(matches!(
        fixture
            .service
            .inspect_conversation(
                &forbidden,
                &binding.binding_id,
                &conversation.conversation_id
            )
            .await,
        Err(BindingError::Forbidden)
    ));
}
