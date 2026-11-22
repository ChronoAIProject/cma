use super::*;

async fn retire_owner(
    store: &MongoProfileStore,
    locator: &ProfileLocator,
    owner: &SubjectId,
    at: &str,
) -> Result<(), ProfileStoreError> {
    match locator {
        ProfileLocator::Agent(id) => store.retire_agent_profile(id, owner, at.into()).await,
        ProfileLocator::Workspace(id) => store.retire_workspace_profile(id, owner, at.into()).await,
        ProfileLocator::Sandbox(id) => store.retire_sandbox_profile(id, owner, at.into()).await,
    }
}

async fn profiles(store: &MongoProfileStore, suffix: &str) -> Vec<ProfileLocator> {
    let agent = agent_profile(
        &format!("agp_{suffix}"),
        "owner",
        ProfileVisibility::Private,
    );
    let workspace = workspace_profile(
        &format!("wkp_{suffix}"),
        "owner",
        ProfileVisibility::Private,
    );
    let sandbox = sandbox_profile(
        &format!("sbp_{suffix}"),
        "owner",
        ProfileVisibility::Private,
    );
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    vec![
        ProfileLocator::Agent(agent.id),
        ProfileLocator::Workspace(workspace.id),
        ProfileLocator::Sandbox(sandbox.id),
    ]
}

async fn stored(client: &Client, database: &str, locator: &ProfileLocator) -> Document {
    let collection = match locator {
        ProfileLocator::Agent(_) => "agent_profiles",
        ProfileLocator::Workspace(_) => "workspace_profiles",
        ProfileLocator::Sandbox(_) => "sandbox_profiles",
    };
    client
        .database(database)
        .collection::<Document>(collection)
        .find_one(doc! {"_id":locator.as_str()})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn first_retirement_survives_owner_retries_and_operator_interleavings() {
    let (client, name, store) = database("owner_order").await;
    let owner = SubjectId("owner".into());
    for operator_first in [false, true] {
        for locator in profiles(&store, if operator_first { "operator" } else { "owner" }).await {
            assert_eq!(
                retire_owner(&store, &locator, &SubjectId("other".into()), T1).await,
                Err(ProfileStoreError::NotFound)
            );
            if operator_first {
                store.retire_profile(&locator, T2.into()).await.unwrap();
            } else {
                retire_owner(&store, &locator, &owner, T2).await.unwrap();
            }
            let first = stored(&client, &name, &locator).await;
            for at in [T3, T1, T2] {
                retire_owner(&store, &locator, &owner, at).await.unwrap();
                assert_eq!(stored(&client, &name, &locator).await, first);
                let summary = store.retire_profile(&locator, at.into()).await.unwrap();
                assert_eq!(summary.retired_at.as_deref(), Some(T2));
                assert_eq!(summary.updated_at, T2);
            }
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_owner_and_operator_retirement_preserve_the_winner() {
    let (client, name, store) = database("owner_race").await;
    let owner = SubjectId("owner".into());
    for locator in profiles(&store, "race").await {
        let (a, b) = tokio::join!(
            retire_owner(&store, &locator, &owner, T2),
            store.retire_profile(&locator, T3.into()),
        );
        a.unwrap();
        b.unwrap();
        let first = stored(&client, &name, &locator).await;
        let at = first.get_str("retired_at").unwrap();
        assert!([T2, T3].contains(&at));
        assert_eq!(first.get_str("updated_at").unwrap(), at);
        let record = first.get_document("record").unwrap();
        assert_eq!(record.get_str("retired_at").unwrap(), at);
        assert_eq!(record.get_str("updated_at").unwrap(), at);
        retire_owner(&store, &locator, &owner, T1).await.unwrap();
        assert_eq!(stored(&client, &name, &locator).await, first);
    }
    client.database(&name).drop().await.unwrap();
}
