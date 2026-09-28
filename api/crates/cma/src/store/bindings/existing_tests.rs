use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::sandbox::binding::{
    BindingState, BindingWork, ExistingBindingAgent, EXISTING_AGENT_CONVERSATION,
};
use layer2_core::{AgentId, WorktreeId};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_existing_agent_migrates_global_index_and_keeps_each_binding_mapping_unique() {
    let (client, database_name, store) = fixture().await;
    let database = client.database(&database_name);
    // Simulate the pre-target schema, reopen, and prove the migration preserves
    // the old association while allowing a second binding to borrow its Agent.
    store
        .bindings
        .conversations
        .drop_index("binding_generation_agent")
        .await
        .unwrap();
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
    reopened
        .save_binding_conversation(second.revision, None, second_conversation.clone())
        .await
        .unwrap();
    for (binding, conversation) in [
        (&first, &first_conversation),
        (&second, &second_conversation),
    ] {
        assert_eq!(
            reopened
                .binding_conversation_for_agent(
                    &binding.binding_id,
                    binding.generation,
                    &conversation.agent_id
                )
                .await
                .unwrap()
                .as_ref(),
            Some(conversation)
        );
        assert_eq!(conversation.conversation_id, EXISTING_AGENT_CONVERSATION);
    }
    assert_eq!(
        reopened
            .save_binding_conversation(second.revision, None, second_conversation)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let indices: Vec<_> = reopened
        .bindings
        .conversations
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(!indices.iter().any(|index| index
        .options
        .as_ref()
        .and_then(|options| options.name.as_deref())
        == Some("record.agent_id_1")));
    assert!(indices.iter().any(|index| index
        .options
        .as_ref()
        .and_then(|options| options.name.as_deref())
        == Some("binding_generation_agent")));
    database.drop().await.unwrap();
}
