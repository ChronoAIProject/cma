use super::*;

#[tokio::test]
async fn model_credit_refuses_before_fencing_and_preserves_interrupt() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let existing = AgentOperationService::new(store.clone(), registry.clone());
    let required = existing.with_model_credit_required(true);
    assert_eq!(
        required
            .send_message(&owner(), &ready.id, "new turn".into())
            .await,
        Err(AgentOperationError::ModelFundingUnsupported)
    );
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
    assert!(matches!(
        existing
            .send_message(&owner(), &ready.id, "existing behavior".into())
            .await
            .unwrap(),
        AgentMessageOutcome::Dispatched(_)
    ));
    required.interrupt(&owner(), &ready.id).await.unwrap();
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [
            ConversationInput::RecoverablePrompt { .. },
            ConversationInput::Cancel
        ]
    ));
}
