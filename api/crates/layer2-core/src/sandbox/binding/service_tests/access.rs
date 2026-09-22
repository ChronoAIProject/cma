use super::*;

#[tokio::test]
async fn binding_keys_and_aliases_are_isolated_even_for_the_same_owner() {
    let mut f = fixture();
    let binding = f.ready().await;
    let access = BindingAccessService::new(f.store.clone());
    let principal = access.authenticate(&f.key.sha256()).await.unwrap().unwrap();
    assert_eq!(
        principal.caller().authorization,
        Authorization::new([Capability::UseBindingConversations])
    );
    let (mut conversation, _) = f
        .service
        .ensure_conversation(&f.caller, &binding.binding_id, "conversation_access")
        .await
        .unwrap();
    assert!(matches!(
        access
            .resolve(&principal, &conversation.conversation_id)
            .await,
        Err(BindingError::Conflict)
    ));
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    f.store
        .save_binding_conversation(binding.revision, Some(1), conversation.clone())
        .await
        .unwrap();
    let proof = access
        .resolve(&principal, &conversation.conversation_id)
        .await
        .unwrap();
    assert_eq!(
        access
            .resolve(&principal, conversation.agent_id.as_str())
            .await
            .unwrap(),
        proof
    );
    assert!(matches!(
        access.resolve(&principal, "conversation_missing").await,
        Err(BindingError::NotFound)
    ));

    // Shared key hashes are refused instead of choosing an arbitrary binding.
    let profile = profile("Another instruction");
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    assert!(matches!(
        f.service
            .put(&f.caller, "binding-two", profile.clone(), &digest, &f.key)
            .await,
        Err(BindingError::Conflict)
    ));
    f.key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let other = f
        .service
        .put(&f.caller, "binding-two", profile, &digest, &f.key)
        .await
        .unwrap();
    let mut ready = other.clone();
    ready.state = BindingState::Ready;
    ready.work = BindingWork::Idle;
    ready.advance_revision(crate::now_rfc3339()).unwrap();
    f.store
        .save_binding(Some(other.revision), ready)
        .await
        .unwrap();
    let other = access.authenticate(&f.key.sha256()).await.unwrap().unwrap();
    assert_eq!(other.owner(), principal.owner());
    assert!(matches!(
        access.resolve(&other, conversation.agent_id.as_str()).await,
        Err(BindingError::NotFound)
    ));
    assert!(matches!(
        access.resolve(&other, &conversation.conversation_id).await,
        Err(BindingError::NotFound)
    ));

    f.service
        .retire_conversation(
            &f.caller,
            &binding.binding_id,
            &conversation.conversation_id,
        )
        .await
        .unwrap();
    assert!(access.validate(&proof).await.is_err());
    f.service
        .retire(&f.caller, &binding.binding_id)
        .await
        .unwrap();
    assert!(access
        .authenticate(principal.key_sha256())
        .await
        .unwrap()
        .is_none());
    assert!(access
        .retained_principal(principal.binding_id(), principal.generation())
        .await
        .is_err());
}
