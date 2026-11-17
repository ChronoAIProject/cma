use super::*;

macro_rules! summary_contract {
    ($name:ident, $make:expr, $create:ident, $publish:expr, $list:ident, $summaries:ident, $revisions:ident) => {
        #[tokio::test]
        async fn $name() {
            let store = InMemoryProfileStore::default();
            let owner = SubjectId("owner".into());
            assert!(store
                .$summaries(&owner, &ProfilePageRequest::default())
                .await
                .unwrap()
                .items
                .is_empty());
            for number in 0..6 {
                let mut profile = ($make)(&format!("profile-{number}"));
                profile.internal = number == 4;
                if number == 5 {
                    profile.owner = SubjectId("foreign".into());
                }
                profile.visibility = if number == 2 || number == 5 {
                    ProfileVisibility::Public
                } else {
                    ProfileVisibility::Private
                };
                store.$create(profile.clone()).await.unwrap();
                for _ in 0..number.min(3) {
                    ($publish)(&store, &profile).await;
                }
            }
            let mut cursor = None;
            let mut total = 0;
            loop {
                let page = ProfilePageRequest { limit: 2, cursor };
                let old = store.$list(Some(&owner), false, &page).await.unwrap();
                let summaries = store.$summaries(&owner, &page).await.unwrap();
                assert_eq!(summaries.next, old.next);
                assert_eq!(summaries.items.len(), old.items.len());
                for (summary, profile) in summaries.items.iter().zip(old.items) {
                    assert_eq!(summary.profile, profile);
                    let latest = store
                        .$revisions(
                            &profile.id,
                            &ProfilePageRequest {
                                limit: 1,
                                cursor: None,
                            },
                        )
                        .await
                        .unwrap();
                    assert_eq!(
                        summary.latest_revision,
                        latest
                            .items
                            .first()
                            .map(crate::profiles::store::PublishedProfileSummary::from)
                    );
                }
                total += summaries.items.len();
                cursor = summaries.next;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(total, 4);
            for limit in [0, 201] {
                assert!(matches!(
                    store
                        .$summaries(
                            &owner,
                            &ProfilePageRequest {
                                limit,
                                cursor: None
                            }
                        )
                        .await,
                    Err(ProfileStoreError::Validation(_))
                ));
            }
        }
    };
}

async fn publish_agent(store: &InMemoryProfileStore, profile: &AgentProfile) {
    store
        .publish_agent_profile(&profile.id, 1, NOW.into())
        .await
        .unwrap();
}
async fn publish_workspace(store: &InMemoryProfileStore, profile: &WorkspaceProfile) {
    store
        .publish_workspace_profile(&profile.id, 1, NOW.into())
        .await
        .unwrap();
}
async fn publish_sandbox(store: &InMemoryProfileStore, profile: &SandboxProfile) {
    store
        .publish_sandbox_profile(&profile.id, 1, NOW.into(), bounds(), &([] as [String; 0]))
        .await
        .unwrap();
}

summary_contract!(
    agent_summary_pages_match_owner_lists,
    |id: &str| agent_profile(id, "owner", ProfileVisibility::Private),
    create_agent_profile,
    publish_agent,
    list_agent_profiles,
    list_agent_profile_summaries,
    list_agent_profile_revisions
);
summary_contract!(
    workspace_summary_pages_match_owner_lists,
    |id: &str| {
        let mut p = workspace_profile(
            id,
            "owner",
            AgentProfileRef::new(AgentProfileId("unused".into()), 1),
        );
        p.draft.agent_profiles.clear();
        p
    },
    create_workspace_profile,
    publish_workspace,
    list_workspace_profiles,
    list_workspace_profile_summaries,
    list_workspace_profile_revisions
);
summary_contract!(
    sandbox_summary_pages_match_owner_lists,
    |id: &str| {
        let mut p = sandbox_profile(
            id,
            "owner",
            WorkspaceProfileRef::new(WorkspaceProfileId("unused".into()), 1),
        );
        p.draft.workspace_profiles.clear();
        p
    },
    create_sandbox_profile,
    publish_sandbox,
    list_sandbox_profiles,
    list_sandbox_profile_summaries,
    list_sandbox_profile_revisions
);
