//! Runs only against the existing disposable Mongo replica-set harness.
use super::associations::{index_keys, INDEX};
use super::*;
use crate::store::sandbox_mongo::{start_transaction, MongoSandboxStore};
use layer2_core::{
    resource_access::ResourceAccessStore,
    sandbox::{
        binding::{BindingConversationState, BindingState, BindingWork, ExistingBindingAgent},
        provider_associations::*,
        store::PageRequest,
    },
    AgentId, SubjectId, WorktreeId,
};

async fn attach(
    store: &MongoSandboxStore,
    agent: &layer2_core::sandbox::Agent,
    id: &str,
) -> BindingConversation {
    let mut binding = super::mongo_tests::binding();
    binding.binding_id = id.into();
    binding.key_sha256 = layer2_core::profiles::content_digest(&id).unwrap();
    binding.owner = agent.owner.clone();
    binding.profile.owner = agent.owner.clone();
    binding.sandbox_id = agent.sandbox_id.clone();
    binding.workspace_id = agent.workspace_id.clone();
    binding.existing_agent = Some(ExistingBindingAgent {
        agent_id: agent.id.clone(),
        worktree_id: agent.worktree_id.clone(),
    });
    binding.level = 1;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    store.save_binding(None, binding.clone()).await.unwrap();
    let conversation = BindingConversation::new(
        &binding,
        "ignored-existing".into(),
        super::mongo_tests::NOW.into(),
    )
    .unwrap();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    conversation
}
async fn retire(store: &MongoSandboxStore, mut conversation: BindingConversation) {
    let mut parent = store
        .binding(&conversation.binding_id)
        .await
        .unwrap()
        .unwrap();
    for state in [
        BindingConversationState::Retiring,
        BindingConversationState::Retired,
    ] {
        let expected = conversation.revision;
        conversation.revision += 1;
        conversation.state = state;
        store
            .save_binding_conversation(parent.revision, Some(expected), conversation.clone())
            .await
            .unwrap();
    }
    let expected = parent.revision;
    parent.state = BindingState::Retired;
    parent
        .advance_revision(super::mongo_tests::NOW.into())
        .unwrap();
    store.save_binding(Some(expected), parent).await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_associations_index_owner_agent_paging_retirement_and_missing_parent() {
    let (client, database, _) = super::mongo_tests::fixture().await;
    let (store, target) =
        crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
            &client, &database,
        )
        .await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let agent = before.lineage.agent.as_ref().unwrap();
    let first = attach(&store, agent, "binding-a").await;
    attach(&store, agent, "binding-b").await;
    let mut other_agent = agent.clone();
    other_agent.id = AgentId("agt_other".into());
    other_agent.worktree_id = WorktreeId("wkt_other".into());
    attach(&store, &other_agent, "binding-0-other-agent").await;
    let mut foreign = agent.clone();
    foreign.owner = SubjectId::new("foreign-owner");
    attach(&store, &foreign, "binding-0-foreign-owner").await;
    let indexes: Vec<_> = store
        .bindings
        .conversations
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let index = indexes
        .iter()
        .find(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some(INDEX)
        })
        .unwrap();
    assert_eq!(index.keys, index_keys());
    assert_ne!(
        index.options.as_ref().and_then(|options| options.unique),
        Some(true)
    );
    let page = PageRequest::new(1, None).unwrap();
    let snapshot = store
        .agent_provider_associations(&agent.owner, &agent.id, &page)
        .await
        .unwrap();
    assert_eq!(snapshot.records.len(), 2); // exactly one bounded look-ahead row
    let first_page = association_page(&snapshot, &agent.owner, &agent.id, &page).unwrap();
    assert_eq!(first_page.items[0].binding_id, "binding-a");
    assert_eq!(
        snapshot,
        store
            .agent_provider_associations(&agent.owner, &agent.id, &page)
            .await
            .unwrap()
    );
    let page = PageRequest::new(1, first_page.next).unwrap();
    let second = store
        .agent_provider_associations(&agent.owner, &agent.id, &page)
        .await
        .unwrap();
    assert_eq!(second.records.len(), 1);
    assert_eq!(
        association_page(&second, &agent.owner, &agent.id, &page)
            .unwrap()
            .items[0]
            .binding_id,
        "binding-b"
    );
    assert!(store
        .agent_provider_associations(&foreign.owner, &agent.id, &page)
        .await
        .is_err());
    retire(&store, first).await;
    let page = PageRequest::new(50, None).unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let snapshot = reopened
        .agent_provider_associations(&agent.owner, &agent.id, &page)
        .await
        .unwrap();
    let read = association_page(&snapshot, &agent.owner, &agent.id, &page).unwrap();
    assert_eq!(read.items.len(), 2);
    assert_eq!(
        read.items[0].conversation_state,
        BindingConversationState::Retired
    );
    assert_eq!(
        read.items[0].binding_state,
        ProviderAssociationBindingState::Retired
    );
    assert_eq!(
        before.lineage,
        reopened
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage
    );
    // A missing parent is corruption, never proof of no membership.
    reopened
        .bindings
        .records
        .delete_one(doc! {"_id":"binding-a"})
        .await
        .unwrap();
    assert!(matches!(
        reopened
            .agent_provider_associations(&agent.owner, &agent.id, &page)
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&database).drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_associations_read_one_snapshot_across_retirement_and_require_index() {
    let (client, database, _) = super::mongo_tests::fixture().await;
    let (store, target) =
        crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
            &client, &database,
        )
        .await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let agent = before.lineage.agent.as_ref().unwrap();
    let conversation = attach(&store, agent, "binding-a").await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    // Pin the transaction snapshot before another connection retires both rows.
    assert!(
        crate::store::agent_provenance::read_snapshot(&store, &agent.id, &mut session)
            .await
            .is_ok(),
        "the fixture must establish its provenance snapshot before retirement"
    );
    retire(&store, conversation).await;
    let page = PageRequest::new(50, None).unwrap();
    let held = super::associations::read(&store, &agent.owner, &agent.id, &page, &mut session)
        .await
        .unwrap();
    assert_eq!(held.records[0].binding.state, BindingState::Ready);
    assert_eq!(
        held.records[0].conversation.state,
        BindingConversationState::Ready
    );
    session.abort_transaction().await.unwrap();
    let current = store
        .agent_provider_associations(&agent.owner, &agent.id, &page)
        .await
        .unwrap();
    assert_eq!(current.records[0].binding.state, BindingState::Retired);
    assert_eq!(
        current.records[0].conversation.state,
        BindingConversationState::Retired
    );
    store
        .bindings
        .conversations
        .drop_index(INDEX)
        .await
        .unwrap();
    assert!(matches!(
        store
            .agent_provider_associations(&agent.owner, &agent.id, &page)
            .await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    client.database(&database).drop().await.unwrap();
}
