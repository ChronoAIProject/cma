use super::*;

#[tokio::test]
async fn bounded_event_windows_keep_frozen_head_and_validate_each_exact_interval() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let first = reads.event_stream_window(&held, 0, None, 2).await.unwrap();
    assert_eq!(first.entries.len(), 2);
    let through = first.through;
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::TitleSet {
                title: "After captured head".into(),
                source: "owner".into(),
            },
        )
        .await
        .unwrap();
    let mut current = agent.clone();
    current.conversation.last_seq += 1;
    current.revision += 1;
    store.save_agent(agent.revision, current).await.unwrap();
    let mut after = 2;
    while after < through {
        let page = reads
            .event_stream_window(&held, after, Some(through), 2)
            .await
            .unwrap();
        assert_eq!(page.through, through);
        assert!(page.entries.len() <= 2);
        for entry in page.entries {
            assert_eq!(entry.sequence, after + 1);
            after += 1;
        }
    }
    let tail = reads
        .event_stream_window(&held, after, None, 2)
        .await
        .unwrap();
    assert_eq!(tail.entries.len(), 1);
    assert_eq!(tail.entries[0].sequence, through + 1);
    for (after, through, limit) in [
        (0, None, 0),
        (0, None, 201),
        (999, None, 2),
        (0, Some(999), 2),
    ] {
        assert!(reads
            .event_stream_window(&held, after, through, limit)
            .await
            .is_err());
    }
    let mut log = store.agent_log(&agent.id, 0, 200).await.unwrap();
    log.remove(3);
    store.replace_agent_log_for_test(&agent.id, log);
    assert!(matches!(
        reads.event_stream_window(&held, 2, Some(through), 2).await,
        Err(AgentReadError::Corrupt(_))
    ));
}

#[tokio::test]
async fn bounded_event_windows_keep_withheld_cursors_and_fence_revoked_policy() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let share = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap()
        .share;
    let reads = service(store).with_resource_access(Arc::new(access.clone()));
    let held = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let page = reads
        .event_stream_window(&held, 0, None, 200)
        .await
        .unwrap();
    assert_eq!(page.entries.len() as u64, page.through);
    assert!(page.entries[0].kind.is_none());
    for (offset, entry) in page.entries.iter().enumerate() {
        assert_eq!(entry.sequence, offset as u64 + 1);
        assert!(!serde_json::to_string(&entry.kind)
            .unwrap()
            .contains("private-"));
    }
    access
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    assert!(reads.event_stream_header(&held).await.is_err());
    assert!(reads.event_stream_window(&held, 0, None, 1).await.is_err());
}

#[tokio::test]
async fn bounded_event_windows_expire_without_renewing_membership() {
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
    tokio::time::pause();
    tokio::time::advance(MAX_MEMBERSHIP_AGE).await;
    assert!(reads.event_stream_window(&held, 0, None, 1).await.is_err());
    assert_eq!(authority.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn bounded_event_windows_refuse_short_or_orphaned_committed_history() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let original = store.agent_log(&agent.id, 0, 200).await.unwrap();
    store.replace_agent_log_for_test(&agent.id, original[..original.len() - 1].to_vec());
    assert!(reads
        .event_stream_window(&held, agent.conversation.last_seq - 1, None, 2)
        .await
        .is_err());
    store.replace_agent_log_for_test(&agent.id, original);
    store
        .append_agent_log(&agent.id, NOW.into(), AgentLogKind::Deleted)
        .await
        .unwrap();
    assert!(reads.event_stream_window(&held, 0, None, 1).await.is_err());
}
