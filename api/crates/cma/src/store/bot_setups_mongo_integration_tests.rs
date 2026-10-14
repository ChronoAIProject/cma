use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::bot_setups::{SetupConfiguration, SetupDraft, SetupStep};
use layer2_core::profiles::{
    AgentProfileContent, ModelAccess, PermissionLevel, SandboxProfileContent, ThinkingEffort,
    WorkspaceLocation, WorkspaceProfileContent,
};

fn setup() -> BotSetup {
    BotSetup {
        id: uuid::Uuid::new_v4().to_string(),
        actor: ExternalActor {
            binding_id: "binding-fixture".into(),
            channel_id: "channel-fixture".into(),
            conversation_id: "conversation-fixture".into(),
            actor_id: "actor-fixture".into(),
            platform: "telegram".into(),
        },
        revision: 1,
        step: SetupStep::Login,
        grant_id: None,
        owner: None,
        draft: SetupDraft::default(),
        configuration: SetupConfiguration {
            agent: AgentProfileContent {
                isolated_agent: false,
                model: Some("fixture-model".into()),
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: "Private fixture instructions".into(),
            },
            workspace: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "work".into(),
                },
                name: "work".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
            sandbox: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1000,
                memory_mib: 2048,
                nyxid_services: vec![],
                agent_provider: Default::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 1,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
            availability: Default::default(),
        },
        selected_services: vec![],
        external_bot_id: None,
        connection_ref: None,
        pending_connection: None,
        admission: None,
        service_page: 0,
        confirmation_digest: None,
        confirmation_event: None,
        bot_id: None,
        processed_actions: vec![],
        processed_events: vec!["fixture-event-1".into()],
        updated_at: layer2_core::now_rfc3339(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn bot_setup_schema_and_encrypted_cas_survive_store_reopen() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database = format!("cma_bot_setup_test_{}", uuid::Uuid::new_v4().simple());
    let key = AuthEncryptionKey::from_base64(&STANDARD.encode([37_u8; 32])).unwrap();
    let original = setup();
    let store = MongoBotSetupStore::open(&client, &database, "fixture", &key)
        .await
        .unwrap();
    store.insert(&original).await.unwrap();
    drop(store);
    drop(client);

    // Reopening runs the shared schema scan against a nonempty collection.
    let client = Client::with_uri_str(&uri).await.unwrap();
    let reopened = MongoBotSetupStore::open(&client, &database, "fixture", &key)
        .await
        .unwrap();
    let loaded = reopened.get(&original.actor).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    let records = client
        .database(&database)
        .collection::<mongodb::bson::Document>("bot_setups");
    let raw = records
        .find_one(doc! {"_id":original.actor.key()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        raw.get_i64("document_schema_version").unwrap(),
        DOCUMENT_SCHEMA_VERSION
    );
    assert!(!raw.contains_key("schema_version"));
    assert!(!format!("{raw:?}").contains("Private fixture instructions"));
    let mut other_actor = original.actor.clone();
    other_actor.actor_id = "another-actor".into();
    assert!(reopened.get(&other_actor).await.unwrap().is_none());

    let mut advanced = loaded;
    advanced.revision += 1;
    advanced.step = SetupStep::Owner;
    advanced.grant_id = Some("grant-fixture".into());
    advanced.processed_events.push("fixture-event-2".into());
    reopened
        .replace(&advanced, original.revision)
        .await
        .unwrap();
    assert!(matches!(
        reopened.replace(&advanced, original.revision).await,
        Err(BotError::Conflict)
    ));
    drop(reopened);

    let reopened = MongoBotSetupStore::open(&client, &database, "fixture", &key)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(reopened.get(&original.actor).await.unwrap().unwrap()).unwrap(),
        serde_json::to_value(advanced).unwrap()
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn gateway_interaction_schema_and_cas_survive_store_reopen() {
    use super::super::gateway_interactions::MongoGatewayInteractionStore;
    use layer2_core::{gateway_interactions::GatewayInteractionStore, SubjectId};

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database = format!("cma_interaction_test_{}", uuid::Uuid::new_v4().simple());
    let owner = SubjectId::new("owner-fixture");
    let expiry = mongodb::bson::DateTime::now().timestamp_millis() / 1000 + 3600;
    let id = uuid::Uuid::new_v4().to_string();
    let store = MongoGatewayInteractionStore::open(&client, &database)
        .await
        .unwrap();
    let first = store
        .put(&id, &owner, b"sealed-fixture".to_vec(), None, expiry)
        .await
        .unwrap();
    drop(store);
    drop(client);

    let client = Client::with_uri_str(&uri).await.unwrap();
    let reopened = MongoGatewayInteractionStore::open(&client, &database)
        .await
        .unwrap();
    assert_eq!(reopened.get(&id).await.unwrap(), Some(first));
    let updated = reopened
        .put(
            &id,
            &owner,
            b"updated-sealed-fixture".to_vec(),
            Some(1),
            expiry,
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert!(matches!(
        reopened
            .put(&id, &owner, b"stale-fixture".to_vec(), Some(1), expiry)
            .await,
        Err(StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        })
    ));
    drop(reopened);
    let reopened = MongoGatewayInteractionStore::open(&client, &database)
        .await
        .unwrap();
    assert_eq!(reopened.get(&id).await.unwrap(), Some(updated));
    client.database(&database).drop().await.unwrap();
}
