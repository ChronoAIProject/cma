use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_sandbox_admission_reads_catalogue_and_fenced_pin_creates_no_resource() {
    let seed = seeded("round2_hotpath_sandbox").await;
    let observed = Commands::default();
    let client = observed.client().await;
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &seed.database)
            .await
            .unwrap(),
    );
    let service = SandboxService::new(
        store,
        seed.profiles.clone(),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = Caller::new(
        SubjectId::new("visitor"),
        Authorization::new([Capability::CreateSandbox]),
    );
    let revision = seed
        .profiles
        .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let request = |name: &str| CreateSandbox {
        profile: seed.target.sandbox_profile.clone(),
        name: Some(name.into()),
        confirmed_digest: Some(revision.digest.clone()),
        origin: SandboxOrigin::Manual,
    };
    observed.clear();
    service.create(&caller, request("admitted")).await.unwrap();
    observed.assert_writes(&[]);
    let db = seed.client.database(&seed.database);
    for (collection, id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        observed.clear();
        assert!(service.create(&caller, request(collection)).await.is_err());
        observed.assert_writes(&[]);
        assert_eq!(
            db.collection::<Document>("sandboxes")
                .count_documents(doc! {"owner":"visitor"})
                .await
                .unwrap(),
            1
        );
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$unset":{"account_erasure_fenced":""}},
            )
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_bot_admission_reads_catalogue_without_changing_published_profiles() {
    use layer2_core::bots::{BotError, BotStore};
    use layer2_core::trigger_domain::TriggerProfileReader;
    let seed = seeded("round2_hotpath_bot").await;
    let observed = Commands::default();
    let client = observed.client().await;
    let store = crate::store::bots::MongoBotStore::open(&client, &seed.database)
        .await
        .unwrap();
    let resolved = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    let mut bot = crate::store::bots_mongo_integration_tests::bot("visitor", "a");
    bot.request.profile = seed.target.agent_profile.clone();
    bot.profiles.agent = resolved.agent;
    bot.profiles.workspace = resolved.workspace;
    bot.profiles.sandbox = resolved.sandbox;
    observed.clear();
    store.insert_catalogue(&bot).await.unwrap();
    observed.assert_writes(&[]);
    let db = seed.client.database(&seed.database);
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    let mut refused = bot.clone();
    refused.bot_id = format!("bot_{}", uuid::Uuid::new_v4().simple());
    refused.creation_key_hash = "c".repeat(64);
    observed.clear();
    assert_eq!(
        store.insert_catalogue(&refused).await,
        Err(BotError::Conflict)
    );
    observed.assert_writes(&[]);
    assert!(store.get(&refused.bot_id).await.unwrap().is_none());
    db.drop().await.unwrap();
}

struct NoRuntime;
#[async_trait::async_trait]
impl layer2_core::sandbox::AgentRuntimeRegistry for NoRuntime {
    async fn resolve(
        &self,
        _: &layer2_core::sandbox::AgentRuntimeTarget,
    ) -> Result<layer2_core::sandbox::AgentRuntimeConnection, layer2_core::sandbox::AgentRuntimeError>
    {
        Err(layer2_core::sandbox::AgentRuntimeError::NotFound)
    }
    async fn fence(
        &self,
        _: &layer2_core::AgentId,
        _: u64,
    ) -> Result<(), layer2_core::sandbox::AgentRuntimeError> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_workspace_and_agent_admissions_only_write_owned_resources() {
    use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};
    use layer2_core::sandbox::{
        AgentOrigin, AgentService, CreateAgent, CreateWorkspace, WorkspaceService, WorkspaceStatus,
        WorkspaceWorkState, WorkspaceWorkStep,
    };
    let seed = seeded("round2_hotpath_workspace_agent").await;
    let observed = Commands::default();
    let client = observed.client().await;
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &seed.database)
            .await
            .unwrap(),
    );
    let mut parent = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_readonly_parent",
    );
    parent.owner = SubjectId::new("visitor");
    store.create_sandbox(parent.clone()).await.unwrap();
    let caller = Caller::new(
        parent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let workspaces = WorkspaceService::new(store.clone(), seed.profiles.clone());
    observed.clear();
    let mut workspace = workspaces
        .create(
            &caller,
            &parent.id,
            CreateWorkspace {
                profile: seed.target.workspace_profile.clone(),
                confirmed_digest: None,
            },
        )
        .await
        .unwrap();
    observed.assert_writes(&[]);
    let expected = workspace.revision;
    workspace.revision += 1;
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(NOW.into());
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    store
        .save_workspace(expected, workspace.clone())
        .await
        .unwrap();
    let agents = AgentService::new(
        store,
        seed.profiles.clone(),
        Arc::new(NoRuntime),
        layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
    );
    let request = CreateAgent {
        profile: seed.target.agent_profile.clone(),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
    };
    observed.clear();
    agents
        .create(&caller, &workspace.id, request.clone())
        .await
        .unwrap();
    observed.assert_writes(&[]);
    let db = seed.client.database(&seed.database);
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    observed.clear();
    assert!(agents
        .create(&caller, &workspace.id, request)
        .await
        .is_err());
    observed.assert_writes(&[]);
    assert_eq!(
        db.collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    observed.clear();
    assert!(workspaces
        .create(
            &caller,
            &parent.id,
            CreateWorkspace {
                profile: seed.target.workspace_profile.clone(),
                confirmed_digest: None,
            }
        )
        .await
        .is_err());
    observed.assert_writes(&[]);
    assert_eq!(
        db.collection::<Document>("workspaces")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
