use super::*;

#[tokio::test]
async fn stream_projects_safe_history_and_revocation_preserves_independent_owner() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access.clone()));
    let owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let share = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap()
        .share;
    let shared = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    assert!(owner.revalidation_deadline().is_none());
    assert!(shared.revalidation_deadline().is_some());
    let snapshot = reads.event_stream_snapshot(&shared).await.unwrap();
    let original = store.agent_log(&agent.id, 0, 200).await.unwrap();
    assert_eq!(snapshot.id, agent.id);
    assert_eq!(snapshot.head, agent.conversation.last_seq);
    assert_eq!(snapshot.history.len(), original.len());
    for (visible, stored) in snapshot.history.iter().zip(&original) {
        assert_eq!(
            (visible.seq, &visible.appended_at),
            (stored.seq, &stored.appended_at)
        );
        assert!(!serde_json::to_string(&visible.kind)
            .unwrap()
            .contains("private-"));
    }
    assert!(snapshot.history[0].kind.is_none());
    access
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    assert!(matches!(
        reads.validate_event_stream(&shared).await,
        Err(AgentReadError::Access(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        )))
    ));
    let personal = reads.event_stream_snapshot(&owner).await.unwrap();
    assert_eq!(personal.history.len(), original.len());
    for (visible, stored) in personal.history.iter().zip(&original) {
        let mut expected = stored.kind.clone();
        if let AgentLogKind::TurnFailed { failure, .. } = &mut expected {
            *failure = crate::sandbox::public_agent_turn_failure(failure);
        }
        assert_eq!(visible.kind.as_ref(), Some(&expected));
    }
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    assert_eq!(store.agent_log(&agent.id, 0, 200).await.unwrap(), original);
}

#[tokio::test]
async fn stream_lifetime_is_monotonic_and_cannot_renew_without_new_authority() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let reads = service(store).with_resource_access(Arc::new(access));
    let held = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let calls = authority.calls.load(Ordering::SeqCst);
    *authority.reply.lock().unwrap() = Reply::Missing;
    tokio::time::pause();
    tokio::time::advance(MAX_MEMBERSHIP_AGE).await;
    assert!(matches!(
        reads.validate_event_stream(&held).await,
        Err(AgentReadError::Access(ResourceAccessError::Access(
            AccessError::RevalidationRequired
        )))
    ));
    assert!(reads.event_stream_snapshot(&held).await.is_err());
    assert_eq!(
        authority.calls.load(Ordering::SeqCst),
        calls,
        "a held stream cannot refresh evidence"
    );
    tokio::time::resume();
    assert!(matches!(
        reads
            .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
            .await,
        Err(AgentReadError::NotFound)
    ));
    *authority.reply.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        reads
            .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
            .await,
        Err(AgentReadError::Access(ResourceAccessError::Membership(
            MembershipError::Unavailable
        )))
    ));
    assert!(reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .is_ok());
}
