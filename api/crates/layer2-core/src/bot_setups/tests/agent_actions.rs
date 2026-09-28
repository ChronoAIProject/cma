use super::*;

fn request(id: &str, view: Option<&SetupView>, action: SetupAction) -> SetupActionRequest {
    SetupActionRequest {
        request_id: id.into(),
        expected: view.map(|view| SetupExpected {
            setup_id: view.setup_id.clone(),
            revision: view.revision,
        }),
        action,
    }
}
async fn act(f: &Fixture, event: &str, request: SetupActionRequest) -> Result<SetupView, BotError> {
    f.service
        .act("binding", "binding-fixture", event, request)
        .await
}
async fn choose(f: &Fixture, event: &str, view: &SetupView, choice: &str) -> SetupView {
    act(
        f,
        event,
        request(
            &format!("choice-{}", view.revision),
            Some(view),
            SetupAction::Choose {
                choice_id: choice.into(),
            },
        ),
    )
    .await
    .unwrap()
}
async fn review(f: &mut Fixture) -> (String, SetupView) {
    let event = f.event(SetupInput::Text(
        "Please make my paper bot personal with all the defaults".into(),
    ));
    let view = act(f, &event, request("view", None, SetupAction::View))
        .await
        .unwrap();
    let view = act(
        f,
        &event,
        request(
            "draft",
            Some(&view),
            SetupAction::Draft {
                draft: SetupDraft {
                    name: Some("Paper assistant".into()),
                    instructions: Some("Explain papers clearly".into()),
                },
            },
        ),
    )
    .await
    .unwrap();
    let mut view = choose(f, &event, &view, "personal").await;
    for choice in [
        "defaults",
        "defaults",
        "defaults",
        "done",
        "bot:external-bot",
        "private",
    ] {
        view = choose(f, &event, &view, choice).await;
    }
    assert_eq!(view.step, SetupStep::Review);
    (event, view)
}

#[tokio::test]
async fn natural_message_supports_many_structured_actions_without_becoming_consent() {
    let mut f = Fixture::new();
    let (event, view) = review(&mut f).await;
    assert_eq!(view.name.as_deref(), Some("Paper assistant"));
    assert_eq!(
        view.configuration.agent.system_prompt,
        "Explain papers clearly"
    );
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
    let create = request(
        "create",
        Some(&view),
        SetupAction::Confirm {
            plan_digest: view.confirmation_digest.clone().unwrap(),
        },
    );
    assert!(matches!(
        act(&f, &event, create.clone()).await,
        Err(BotError::Forbidden)
    ));
    let unrelated = f.event(SetupInput::Text("What is the weather?".into()));
    assert!(matches!(
        act(&f, &unrelated, create).await,
        Err(BotError::Forbidden)
    ));
    let forged_choice = request(
        "forged",
        Some(&view),
        SetupAction::Choose {
            choice_id: format!("create:{}", view.confirmation_digest.as_deref().unwrap()),
        },
    );
    assert!(matches!(
        act(&f, &unrelated, forged_choice).await,
        Err(BotError::Invalid(_))
    ));
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_request_replay_is_unchanged_changed_payload_and_stale_revision_fail() {
    let mut f = Fixture::new();
    let event = f.event(SetupInput::Text("I want a bot".into()));
    let start = request("start", None, SetupAction::View);
    let view = act(&f, &event, start.clone()).await.unwrap();
    let replay = act(&f, &event, start).await.unwrap();
    assert_eq!(view.revision, replay.revision);
    let choose = request(
        "owner",
        Some(&view),
        SetupAction::Choose {
            choice_id: "personal".into(),
        },
    );
    let selected = act(&f, &event, choose.clone()).await.unwrap();
    assert_eq!(selected.step, SetupStep::Name);
    f.service = BotSetupService::new(f.store.clone(), f.dependencies.clone());
    assert_eq!(
        act(&f, &event, choose.clone()).await.unwrap().revision,
        selected.revision
    );
    let mut changed = choose.clone();
    changed.action = SetupAction::Choose {
        choice_id: "org:org".into(),
    };
    assert!(matches!(
        act(&f, &event, changed).await,
        Err(BotError::Conflict)
    ));
    let mut stale = choose;
    stale.request_id = "another-owner-request".into();
    assert!(matches!(
        act(&f, &event, stale).await,
        Err(BotError::Conflict)
    ));
    assert!(matches!(
        act(
            &f,
            &event,
            request(
                "no-revision",
                None,
                SetupAction::Choose {
                    choice_id: "personal".into()
                }
            )
        )
        .await,
        Err(BotError::Invalid(_))
    ));
}

#[tokio::test]
async fn fresh_attested_consent_checks_digest_and_replays_one_provision_after_restart() {
    let mut f = Fixture::new();
    let (_, view) = review(&mut f).await;
    let event = f.event(SetupInput::Text("确认创建".into()));
    let mut create = request(
        "create",
        Some(&view),
        SetupAction::Confirm {
            plan_digest: "forged".into(),
        },
    );
    assert!(matches!(
        act(&f, &event, create.clone()).await,
        Err(BotError::Conflict)
    ));
    create.action = SetupAction::Confirm {
        plan_digest: view.confirmation_digest.clone().unwrap(),
    };
    let created = act(&f, &event, create.clone()).await.unwrap();
    assert_eq!(created.step, SetupStep::Provisioning);
    assert_eq!(f.dependencies.resources.lock().unwrap().len(), 1);
    f.service = BotSetupService::new(f.store.clone(), f.dependencies.clone());
    let replay = act(&f, &event, create).await.unwrap();
    assert_eq!(
        replay.result.unwrap().bot_id,
        created.result.unwrap().bot_id
    );
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn consent_before_review_or_with_expired_interaction_cannot_create() {
    let mut f = Fixture::new();
    let (_, view) = review(&mut f).await;
    let event = f.event(SetupInput::Text("create bot".into()));
    f.dependencies
        .events
        .lock()
        .unwrap()
        .get_mut(&event)
        .unwrap()
        .occurred_at = "2020-01-01T00:00:00Z".into();
    let create = request(
        "create",
        Some(&view),
        SetupAction::Confirm {
            plan_digest: view.confirmation_digest.clone().unwrap(),
        },
    );
    assert!(matches!(
        act(&f, &event, create.clone()).await,
        Err(BotError::Forbidden)
    ));
    let expired = f.event(SetupInput::Interaction {
        block_id: format!("{}.{}", view.setup_id, view.revision - 1),
        action_id: format!("create:{}", view.confirmation_digest.as_deref().unwrap()),
    });
    assert!(matches!(
        act(&f, &expired, create.clone()).await,
        Err(BotError::Forbidden)
    ));
    let valid = f.confirm(&view);
    assert_eq!(
        act(&f, &valid, create).await.unwrap().step,
        SetupStep::Provisioning
    );
}

#[tokio::test]
async fn typed_customization_validates_and_advances_one_component() {
    let mut f = Fixture::new();
    let event = f.event(SetupInput::Text(
        "Make my bot thorough, called Paper assistant".into(),
    ));
    let view = act(&f, &event, request("view", None, SetupAction::View))
        .await
        .unwrap();
    let view = choose(&f, &event, &view, "personal").await;
    let view = act(
        &f,
        &event,
        request(
            "draft",
            Some(&view),
            SetupAction::Draft {
                draft: SetupDraft {
                    name: Some("Paper assistant".into()),
                    instructions: None,
                },
            },
        ),
    )
    .await
    .unwrap();
    let view = choose(&f, &event, &view, "custom").await;
    let mut agent = view.configuration.agent.clone();
    agent.thinking_effort = ThinkingEffort::High;
    let change = SetupAction::Customize {
        customization: Box::new(SetupCustomization {
            agent: Some(agent),
            ..Default::default()
        }),
    };
    let updated = act(&f, &event, request("customize", Some(&view), change))
        .await
        .unwrap();
    assert_eq!(updated.step, SetupStep::Workspace);
    assert_eq!(
        updated.configuration.agent.thinking_effort,
        ThinkingEffort::High
    );
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn agent_actions_keep_owner_catalog_and_binding_authority() {
    let mut f = Fixture::new();
    let event = f.event(SetupInput::Text("Use my organization".into()));
    let view = act(&f, &event, request("view", None, SetupAction::View))
        .await
        .unwrap();
    let forged = request(
        "foreign-owner",
        Some(&view),
        SetupAction::Choose {
            choice_id: "org:someone-else".into(),
        },
    );
    assert!(matches!(
        act(&f, &event, forged).await,
        Err(BotError::Forbidden)
    ));
    let owner = request(
        "owner",
        Some(&view),
        SetupAction::Choose {
            choice_id: "personal".into(),
        },
    );
    assert!(matches!(
        f.service
            .act("other-binding", "binding-fixture", &event, owner.clone())
            .await,
        Err(BotError::Forbidden)
    ));
    assert!(matches!(
        f.service.act("binding", "wrong-key", &event, owner).await,
        Err(BotError::Forbidden)
    ));
}

#[tokio::test]
async fn reads_preserve_review_revision_and_midflow_edits_invalidate_confirmation() {
    let mut f = Fixture::new();
    let (event, view) = review(&mut f).await;
    let stored = f
        .store
        .get(&f.actor)
        .await
        .unwrap()
        .unwrap()
        .confirmation_event
        .unwrap();
    let reread = act(&f, &event, request("read-again", None, SetupAction::View))
        .await
        .unwrap();
    assert_eq!(view.revision, reread.revision);
    let unchanged = f
        .store
        .get(&f.actor)
        .await
        .unwrap()
        .unwrap()
        .confirmation_event
        .unwrap();
    assert_eq!(stored.reviewed_at, unchanged.reviewed_at);
    assert_eq!(stored.event_id, unchanged.event_id);
    let owner = choose(&f, &event, &reread, "edit").await;
    assert_eq!(owner.step, SetupStep::Owner);
    assert!(owner.confirmation_digest.is_none());
    let changed = act(
        &f,
        &event,
        request(
            "rename",
            Some(&owner),
            SetupAction::Draft {
                draft: SetupDraft {
                    name: Some("New title".into()),
                    instructions: None,
                },
            },
        ),
    )
    .await
    .unwrap();
    let agent = choose(&f, &event, &changed, "personal").await;
    assert_eq!(agent.step, SetupStep::Agent);
    let edit = choose(&f, &event, &agent, "edit").await;
    assert_eq!(edit.step, SetupStep::Owner);
    assert_eq!(edit.name.as_deref(), Some("New title"));
}

#[tokio::test]
async fn legacy_advance_cannot_bypass_the_current_review_consent_guard() {
    let mut f = Fixture::new();
    let (_, view) = review(&mut f).await;
    let old = f.event(SetupInput::Text("create".into()));
    f.dependencies
        .events
        .lock()
        .unwrap()
        .get_mut(&old)
        .unwrap()
        .occurred_at = "2020-01-01T00:00:00Z".into();
    assert!(matches!(f.advance(&old).await, Err(BotError::Forbidden)));
    let callback = f.confirm(&view);
    f.dependencies
        .events
        .lock()
        .unwrap()
        .get_mut(&callback)
        .unwrap()
        .occurred_at = "2020-01-01T00:00:00Z".into();
    assert!(matches!(
        f.advance(&callback).await,
        Err(BotError::Forbidden)
    ));
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn defaults_replace_previous_custom_values_but_preserve_user_instructions() {
    let mut f = Fixture::new();
    let (event, review) = review(&mut f).await;
    {
        let mut records = f.store.records.lock().unwrap();
        let setup = records.get_mut(&f.actor.key()).unwrap();
        setup.configuration.agent.thinking_effort = ThinkingEffort::High;
        setup.configuration.workspace.setup_commands = vec!["echo customized".into()];
        setup.configuration.sandbox.cpu_millis = 8000;
        setup.configuration.availability.always_keep_alive = true;
    }
    let owner = choose(&f, &event, &review, "edit").await;
    let mut view = choose(&f, &event, &owner, "personal").await;
    for _ in 0..3 {
        view = choose(&f, &event, &view, "defaults").await;
    }
    assert_eq!(view.step, SetupStep::Services);
    assert_eq!(
        view.configuration.agent.thinking_effort,
        ThinkingEffort::Medium
    );
    assert_eq!(
        view.configuration.agent.system_prompt,
        "Explain papers clearly"
    );
    assert!(view.configuration.workspace.setup_commands.is_empty());
    assert_eq!(view.configuration.sandbox.cpu_millis, 1000);
    assert!(!view.configuration.availability.always_keep_alive);
}
