use super::*;

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
    let drift = inspected
        .profile_drift
        .as_ref()
        .expect("owned Agent profile");
    assert_eq!(drift.assigned_profile_digest, binding.profile_digest);
    assert!(!drift.prompt && !drift.skills && !drift.model);
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

#[tokio::test]
async fn inspection_drift_tracks_owner_prompt_and_installed_skill_changes() {
    let fixture = fixture();
    let binding = fixture.ready().await;
    let conversation = resource_access::ready_conversation(&fixture, &binding).await;
    let mut agent = fixture
        .store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let mut settings = agent.settings();
    settings.system_prompt = Some("Owner changed prompt".into());
    agent.conversation.settings = Some(settings);
    let expected = &binding.profile.agent;
    let drift = super::super::inspection::profile_drift(&agent, expected, &binding.profile_digest);
    assert!(drift.prompt);
    assert!(!drift.skills);
    let mut changed = expected.clone();
    changed.skills.push(crate::profiles::SkillRef {
        source: crate::profiles::SkillSource::Ornn,
        id: crate::SkillId::new("synthetic"),
        version: Some("1.0".into()),
    });
    let drift = super::super::inspection::profile_drift(&agent, &changed, &binding.profile_digest);
    assert!(drift.skills);
}

#[tokio::test]
async fn desired_profile_drift_compares_the_scope_that_binding_admission_would_grant() {
    let fixture = fixture();
    let mut desired = profile("Keep this instruction");
    desired["model"]["service"] = json!(
        AgentProvider::default()
            .connection_services()
            .legacy_managed
    );
    desired["permission"]["service_scope"] = json!(["original-service"]);
    let binding = fixture.ready_with_profile(desired.clone(), None).await;
    let conversation = resource_access::ready_conversation(&fixture, &binding).await;

    for scope in [None, Some(json!(["replacement-service"]))] {
        let scope_changed = scope.is_some();
        if let Some(scope) = scope {
            desired["permission"]["service_scope"] = scope;
        }
        let digest = format!("sha256:{}", content_digest(&desired).unwrap());
        let drift = fixture
            .service
            .desired_profile_drift(
                &fixture.caller,
                &binding.binding_id,
                &conversation.conversation_id,
                desired.clone(),
                &digest,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(drift.profile_digest, digest);
        assert_eq!(drift.service_scope, scope_changed);
        assert!(!drift.prompt && !drift.skills && !drift.model);
    }

    assert_eq!(fixture.binding().await, binding);
}
