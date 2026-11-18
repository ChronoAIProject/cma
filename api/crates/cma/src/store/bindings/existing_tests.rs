use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::sandbox::binding::{BindingState, BindingWork, ExistingBindingAgent};
use layer2_core::{AgentId, WorktreeId};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_existing_agent_preserves_global_index_until_an_explicit_cutover() {
    let (client, database_name, _) = fixture().await;
    let database = client.database(&database_name);
    let store = MongoSandboxStore::open(&client, &database_name)
        .await
        .unwrap();
    // The explicit additive preparation must coexist with the retained global
    // constraint. Ordinary open has no authority to retire that constraint.
    store
        .bindings
        .conversations
        .create_index(
            IndexModel::builder()
                .keys(doc! {"record.agent_id":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let make = |id: &str, hash: &str| {
        let mut value = binding();
        value.binding_id = id.into();
        value.key_sha256 = hash.into();
        value.existing_agent = Some(ExistingBindingAgent {
            agent_id: AgentId("agt_existing".into()),
            worktree_id: WorktreeId("wt_existing".into()),
        });
        value.level = 1;
        value.state = BindingState::Ready;
        value.work = BindingWork::Idle;
        value
    };
    let first = make("first-binding", &"a".repeat(64));
    store.save_binding(None, first.clone()).await.unwrap();
    let first_conversation =
        BindingConversation::new(&first, "transport-one".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(first.revision, None, first_conversation.clone())
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &database_name)
        .await
        .unwrap();
    let second = make("second-binding", &"b".repeat(64));
    reopened.save_binding(None, second.clone()).await.unwrap();
    let second_conversation =
        BindingConversation::new(&second, "transport-two".into(), NOW.into()).unwrap();
    assert_eq!(
        reopened
            .save_binding_conversation(second.revision, None, second_conversation.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        reopened
            .binding_conversation_for_agent(
                &first.binding_id,
                first.generation,
                &first_conversation.agent_id
            )
            .await
            .unwrap(),
        Some(first_conversation.clone())
    );
    assert!(reopened
        .binding_conversation_for_agent(
            &second.binding_id,
            second.generation,
            &second_conversation.agent_id
        )
        .await
        .unwrap()
        .is_none());
    let retained: Vec<_> = reopened
        .bindings
        .conversations
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(retained.iter().any(|index| index
        .options
        .as_ref()
        .and_then(|options| options.name.as_deref())
        == Some("record.agent_id_1")));
    let mut observer =
        crate::store::schema::StartupObserver::new(crate::config::StartupSchemaMode::Enforce);
    assert!(BindingCollections::open(&database, &mut observer)
        .await
        .is_err());
    database.drop().await.unwrap();
}
