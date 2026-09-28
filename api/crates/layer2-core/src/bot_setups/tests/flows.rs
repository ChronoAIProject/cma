use super::*;

#[tokio::test]
async fn text_commands_drive_the_same_durable_workflow_without_relay_callbacks() {
    let mut fixture = Fixture::new();
    let mut view = fixture.begin().await;
    assert_eq!(view.step, SetupStep::Owner);
    view = fixture.text("personal").await;
    assert_eq!(view.step, SetupStep::Agent);
    view = fixture.text("defaults").await;
    assert_eq!(view.step, SetupStep::Workspace);
    view = fixture.text("defaults").await;
    assert_eq!(view.step, SetupStep::Sandbox);
    view = fixture.text("defaults").await;
    assert_eq!(view.step, SetupStep::Services);
    view = fixture.text("done").await;
    assert_eq!(view.step, SetupStep::ExternalBot);
    view = fixture.text("bot:external-bot").await;
    assert_eq!(view.step, SetupStep::Admission);
    view = fixture.text("private").await;
    assert_eq!(view.step, SetupStep::Review);
    let created = fixture.text("create").await;
    assert_eq!(created.step, SetupStep::Provisioning);
}

#[tokio::test]
async fn confirmation_is_explicit_duplicate_safe_and_survives_restart_and_completion() {
    let mut fixture = Fixture::new();
    let review = fixture.review().await;
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        0
    );
    let event = fixture.confirm(&review);
    let created = fixture.advance(&event).await.unwrap();
    assert_eq!(created.step, SetupStep::Provisioning);
    assert_eq!(fixture.dependencies.resources.lock().unwrap().len(), 1);
    fixture.service = BotSetupService::new(fixture.store.clone(), fixture.dependencies.clone());
    let replay = fixture.advance(&event).await.unwrap();
    assert_eq!(
        created.result.unwrap().bot_id,
        replay.result.as_ref().unwrap().bot_id
    );
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        1
    );
    fixture.dependencies.active.store(true, Ordering::SeqCst);
    let completed = fixture.action(&replay, "refresh").await;
    assert_eq!(completed.step, SetupStep::Complete);
    assert_eq!(
        fixture
            .store
            .get(&fixture.actor)
            .await
            .unwrap()
            .unwrap()
            .step,
        SetupStep::Complete
    );
    let next = fixture.action(&completed, "new").await;
    assert_ne!(next.setup_id, completed.setup_id);
    assert!(next.result.is_none());
    let owner = fixture.begin().await;
    assert_eq!(owner.step, SetupStep::Owner);
}

#[tokio::test]
async fn interrupted_result_checkpoint_reuses_one_resource_on_replayed_confirmation() {
    let mut fixture = Fixture::new();
    let review = fixture.review().await;
    let event = fixture.confirm(&review);
    fixture
        .store
        .fail_result_write
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.advance(&event).await,
        Err(BotError::Unavailable)
    ));
    fixture.service = BotSetupService::new(fixture.store.clone(), fixture.dependencies.clone());
    assert_eq!(
        fixture.advance(&event).await.unwrap().step,
        SetupStep::Provisioning
    );
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        2
    );
    assert_eq!(fixture.dependencies.resources.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn organization_scope_connections_and_optional_service_deselection_remain_explicit() {
    let mut fixture = Fixture::new();
    let view = fixture.services("org:org").await;
    let selected = fixture.action(&view, "service:optional-id").await;
    assert!(
        selected
            .services
            .iter()
            .find(|s| s.id == "optional-id")
            .unwrap()
            .selected
    );
    let unselected = fixture.action(&selected, "service:optional-id").await;
    assert!(
        !unselected
            .services
            .iter()
            .find(|s| s.id == "optional-id")
            .unwrap()
            .selected
    );
    let missing = fixture.action(&unselected, "done").await;
    assert_eq!(missing.step, SetupStep::Services);
    assert!(missing.connection_url.is_some());
    assert_eq!(
        *fixture.dependencies.connections.lock().unwrap(),
        vec![(
            ResourceOwner::Organization {
                subject: SubjectId::new("org")
            },
            "managed-id".into()
        )]
    );
    fixture
        .dependencies
        .organization_connected
        .store(true, Ordering::SeqCst);
    let ready = fixture.action(&missing, "done").await;
    assert_eq!(ready.step, SetupStep::ExternalBot);
    assert_eq!(
        ready.configuration.sandbox.nyxid_services,
        vec![crate::ServiceId::new("managed-id")]
    );
}

#[tokio::test]
async fn revoked_grant_and_stale_confirmation_cannot_create_resources() {
    let mut fixture = Fixture::new();
    let review = fixture.review().await;
    let stale = fixture.confirm(&review);
    let edited = fixture.action(&review, "edit").await;
    assert_eq!(edited.step, SetupStep::Owner);
    let stale_response = fixture.advance(&stale).await.unwrap();
    assert_eq!(stale_response.step, SetupStep::Owner);
    assert!(stale_response.notice.is_some());
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        0
    );
    fixture
        .dependencies
        .identities
        .lock()
        .unwrap()
        .get_mut(&fixture.actor.key())
        .unwrap()
        .grant_id = None;
    let reset = fixture.action(&edited, "personal").await;
    assert_eq!(reset.step, SetupStep::Login);
    assert!(reset.owner.is_none());
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn replay_cannot_borrow_another_actors_identity_or_a_revoked_grant() {
    let mut fixture = Fixture::new();
    let review = fixture.review().await;
    let event = fixture.confirm(&review);
    let created = fixture.advance(&event).await.unwrap();
    assert_eq!(created.step, SetupStep::Provisioning);
    fixture
        .dependencies
        .identities
        .lock()
        .unwrap()
        .get_mut(&fixture.actor.key())
        .unwrap()
        .grant_id = None;
    assert!(matches!(
        fixture.advance(&event).await,
        Err(BotError::Forbidden)
    ));
    let mut other_actor = fixture.actor.clone();
    other_actor.actor_id = "mallory".into();
    fixture.dependencies.events.lock().unwrap().insert(
        "foreign".into(),
        SetupEvent {
            occurred_at: crate::now_rfc3339(),
            actor: other_actor.clone(),
            event_id: "foreign".into(),
            input: SetupInput::Interaction {
                block_id: review.interaction_id.clone(),
                action_id: review
                    .choices
                    .iter()
                    .find(|c| c.id.starts_with("create:"))
                    .unwrap()
                    .id
                    .clone(),
            },
        },
    );
    let foreign = fixture.advance("foreign").await.unwrap();
    assert_eq!(foreign.step, SetupStep::Login);
    assert!(foreign.result.is_none());
    assert_eq!(fixture.dependencies.resources.lock().unwrap().len(), 1);
}
