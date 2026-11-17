use super::*;

mod summaries;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_publication_is_transactional_and_revisions_page_newest_first() {
    let (client, name) = database("profiles").await;
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    let profile = agent_profile("agp_publication");
    store.create_agent_profile(profile.clone()).await.unwrap();
    assert_eq!(
        store
            .publish_agent_profile(&profile.id, 1, NOW.into())
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        store
            .publish_agent_profile(&profile.id, 1, NOW.into())
            .await
            .unwrap()
            .revision,
        2
    );
    let first = store
        .list_agent_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items[0].revision, 2);
    assert_eq!(first.next.as_deref(), Some("2"));
    let second = store
        .list_agent_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: first.next,
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items[0].revision, 1);
    let mut stale = profile;
    stale.draft_version = 2;
    assert_eq!(
        store.save_agent_draft(0, stale).await,
        Err(ProfileStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}
