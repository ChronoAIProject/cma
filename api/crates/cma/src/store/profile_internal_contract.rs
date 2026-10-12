//! Executed by the authenticated Agent Manager Mongo fixture.
use super::super::sandbox_mongo::MongoProfileStore;
use layer2_core::profiles::*;
use layer2_core::{AgentProfileId, SubjectId};
use mongodb::{bson::doc, Client};
use std::sync::Arc;

pub(super) async fn verify(client: &Client, database: &str) {
    let store = Arc::new(MongoProfileStore::open(client, database).await.unwrap());
    let bounds = ProfileCapacityBounds {
        min_cpu_millis: 1,
        max_cpu_millis: 4000,
        min_memory_mib: 1,
        max_memory_mib: 8192,
    };
    let scanner = Arc::new(["SECRET_TOKEN".to_owned()]);
    let snapshots = PrivateProfileCreationService::new(store.clone(), bounds, scanner.clone());
    let owner = SubjectId::new("profile-owner");
    let revision = snapshots
        .create_agent_for_owner(
            &owner,
            "inline-operation",
            "Runtime setup".into(),
            String::new(),
            AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        )
        .await
        .unwrap();
    let internal = store
        .agent_profile(&revision.profile_id)
        .await
        .unwrap()
        .unwrap();
    assert!(internal.internal);
    let mut users = Vec::new();
    for id in [
        "agp_00000000000000000000000000000000",
        "agp_ffffffffffffffffffffffffffffffff",
    ] {
        let mut user = internal.clone();
        user.id = AgentProfileId(id.into());
        user.internal = false;
        user.name = "Inline Trigger user configuration".into();
        user.description = "Internal Agent Manager Trigger user description".into();
        store.create_agent_profile(user.clone()).await.unwrap();
        users.push(user);
    }
    let first = store
        .list_agent_profiles(
            Some(&owner),
            false,
            &ProfilePageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items, vec![users[0].clone()]);
    assert_eq!(first.next.as_deref(), Some(users[0].id.as_str()));
    let last = store
        .list_agent_profiles(
            Some(&owner),
            false,
            &ProfilePageRequest {
                limit: 1,
                cursor: first.next,
            },
        )
        .await
        .unwrap();
    assert_eq!(last.items, vec![users[1].clone()]);
    assert!(last.next.is_none());
    let admin = store
        .list_profiles(&ProfileAdminQuery {
            kind: None,
            owner: None,
            visibility: None,
            page: ProfilePageRequest::default(),
        })
        .await
        .unwrap();
    assert_eq!(admin.items.len(), 2);
    for original in [&internal, &users[0]] {
        let mut changed = original.clone();
        changed.internal = !original.internal;
        changed.draft_version += 1;
        assert_eq!(
            store.save_agent_draft(1, changed).await,
            Err(ProfileStoreError::Conflict)
        );
    }
    assert_eq!(
        store
            .retire_agent_profile(&internal.id, &owner, layer2_core::now_rfc3339())
            .await,
        Err(ProfileStoreError::NotFound)
    );
    assert_eq!(
        store
            .retire_profile(
                &ProfileLocator::Agent(internal.id.clone()),
                layer2_core::now_rfc3339()
            )
            .await,
        Err(ProfileStoreError::NotFound)
    );
    assert_eq!(
        store
            .set_profile_visibility(
                &owner,
                &MarketplaceLocator::Agent(internal.id.clone()),
                ProfileVisibility::Public,
                layer2_core::now_rfc3339(),
                bounds,
                scanner.as_ref()
            )
            .await,
        Err(MarketplaceError::NotFound)
    );
    let raw = store
        .agent_profiles
        .find_one(doc! {"_id": users[0].id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!raw.get_document("record").unwrap().contains_key("internal"));
    let restarted = MongoProfileStore::open(client, database).await.unwrap();
    assert!(
        restarted
            .agent_profile(&internal.id)
            .await
            .unwrap()
            .unwrap()
            .internal
    );
    assert!(
        !restarted
            .agent_profile(&users[0].id)
            .await
            .unwrap()
            .unwrap()
            .internal
    );
    assert_eq!(
        restarted
            .agent_profile_revision(&internal.id, 1)
            .await
            .unwrap(),
        Some(revision)
    );
}
