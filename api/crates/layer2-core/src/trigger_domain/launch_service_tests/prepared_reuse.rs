use super::*;

#[tokio::test]
async fn prepared_agent_reopens_via_its_launch_binding_and_keeps_original_provenance() {
    let fixture = fixture(
        ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::ResumeExisting,
        },
        VISITOR,
        false,
        false,
        vec![],
    )
    .await;
    fixture.set_context(ExistingContext::LiveAgent);
    let first = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("initial-adoption"),
        )
        .await
        .unwrap()
        .launch;
    let mut prepared = fixture
        .contexts
        .state
        .lock()
        .unwrap()
        .agent
        .clone()
        .unwrap();
    prepared.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    prepared.creation_key = format!("workspace-profile:{}:0", prepared.workspace_id);
    prepared.conversation.first_message = None;
    prepared.work.warm_conversation_id = prepared.conversation.conversation_id.clone();
    prepared.validate_conversation().unwrap();
    {
        let mut state = fixture.contexts.state.lock().unwrap();
        state.agent = None;
        state
            .prepared_agents
            .insert(prepared.id.clone(), prepared.clone());
    }
    assert_eq!(
        fixture
            .launches
            .source_launch(&fixture.caller.subject, &prepared.id, None)
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    let repeated = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("repeat-prepared"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(repeated.resolution, first.resolution);
    assert_eq!(repeated.first_message, FirstMessageDelivery::Completed);
    assert_eq!(
        fixture.contexts.state.lock().unwrap().prepared_agents[&prepared.id],
        prepared
    );
    assert!(fixture
        .launches
        .source_launch(&SubjectId::new("another-user"), &prepared.id, None)
        .await
        .unwrap()
        .is_none());

    // A stale binding must not reconnect a different workspace with the same Agent ID.
    fixture
        .contexts
        .state
        .lock()
        .unwrap()
        .prepared_agents
        .get_mut(&prepared.id)
        .unwrap()
        .workspace_id = WorkspaceId("wks_wrong".into());
    assert!(matches!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("crossed-prepared")
            )
            .await,
        Err(LaunchServiceError::Corrupt(_))
    ));
}
