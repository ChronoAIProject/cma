use super::*;

struct Instructions;
#[async_trait::async_trait]
impl layer3_adapter::ConversationSettingsSupport for Instructions {
    async fn system_prompt_supported(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn rename_during_work_retains_every_runtime_and_history_field_and_rejects_stale_edits() {
    let store = Arc::new(InMemorySandboxStore::default());
    let working = test_agent(AgentStatus::Working, None);
    seed_active_parent(&store).await;
    store.create_agent(working.clone()).await.unwrap();
    let (sender, calls) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let updated = service
        .update_preferences(
            &owner(),
            &working.id,
            AgentPreferencesUpdate::Title {
                title: "Owner chosen name".into(),
                expected_revision: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.profile, working.profile);
    assert_eq!(updated.title, working.title);
    assert_eq!(updated.status, working.status);
    assert_eq!(
        updated.conversation.active_turn,
        working.conversation.active_turn
    );
    assert_eq!(
        updated.conversation.conversation_id,
        working.conversation.conversation_id
    );
    assert_eq!(
        updated.conversation.runtime_generation,
        working.conversation.runtime_generation
    );
    assert_eq!(
        updated.conversation.preferences.title.as_deref(),
        Some("Owner chosen name")
    );
    assert_eq!(updated.conversation.preferences.revision, 1);
    assert!(calls.lock().unwrap().is_empty());
    assert!(matches!(
        service
            .update_preferences(
                &owner(),
                &working.id,
                AgentPreferencesUpdate::Title {
                    title: "Stale edit".into(),
                    expected_revision: 0,
                }
            )
            .await,
        Err(AgentOperationError::Conflict)
    ));
}

#[tokio::test]
async fn prompt_is_fenced_and_acknowledged_then_survives_a_later_model_change() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let saved = store.clone();
    let id = ready.id.clone();
    let sender = layer3_adapter::ConversationInputSender::spawn(2, move |input| {
        let saved=saved.clone(); let id=id.clone();
        async move {
            let current=saved.get_agent(&id).await.unwrap().unwrap();
            assert!(current.settings_pending());
            assert_eq!(current.conversation.preferences.revision,1);
            assert!(matches!(input, ConversationInput::Configure {system_prompt:Some(prompt),..} if prompt=="Owner revised instructions"));
            Ok(())
        }
    }).with_settings_support(Arc::new(Instructions));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let updated = service
        .update_preferences(
            &owner(),
            &ready.id,
            AgentPreferencesUpdate::SystemPrompt {
                expected_revision: 0,
                system_prompt: "Owner revised instructions".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.system_prompt(), "Owner revised instructions");
    assert!(!updated.settings_pending());
    assert_eq!(updated.profile, ready.profile);
    assert_eq!(
        updated.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    let mut settings = updated.settings();
    settings.model = "different-model".into();
    settings.system_prompt = None;
    let updated = service
        .update_settings(&owner(), &ready.id, settings)
        .await
        .unwrap();
    assert_eq!(updated.system_prompt(), "Owner revised instructions");
    assert_eq!(updated.conversation.preferences.revision, 1);
}

#[tokio::test]
async fn unsupported_or_busy_prompt_edit_leaves_durable_and_runtime_state_untouched() {
    for active in [false, true] {
        let store = Arc::new(InMemorySandboxStore::default());
        let agent = test_agent(
            if active {
                AgentStatus::Working
            } else {
                AgentStatus::Ready
            },
            None,
        );
        seed_active_parent(&store).await;
        store.create_agent(agent.clone()).await.unwrap();
        let (sender, calls) = recording_sender(Ok(()));
        let service = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        assert!(service
            .update_preferences(
                &owner(),
                &agent.id,
                AgentPreferencesUpdate::SystemPrompt {
                    expected_revision: 0,
                    system_prompt: "New instructions".into(),
                }
            )
            .await
            .is_err());
        assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[test]
fn preferences_bound_unicode_title_and_utf8_prompt_without_mutating_original_profiles() {
    let title = |title: String| AgentPreferencesUpdate::Title {
        expected_revision: 0,
        title,
    };
    assert!(title("标".repeat(64)).validate().is_ok());
    assert!(title("标".repeat(65)).validate().is_err());
    assert!(title("name\nline".into()).validate().is_err());
    assert!(AgentPreferencesUpdate::SystemPrompt {
        expected_revision: 0,
        system_prompt: "标".repeat(10923)
    }
    .validate()
    .is_err());
}
