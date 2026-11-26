//! Actual ordinary roots initiate adoption; closed gates retain legacy bytes.
use super::*;
use crate::store::agent_log_segments::{writer_control, writer_gate};
use futures_util::TryStreamExt;
use mongodb::bson::{Document, RawDocumentBuf};
use mongodb::ClientSession;
use std::collections::BTreeMap;

#[path = "writer_caller_unknown_tests.rs"]
mod unknown;

#[path = "writer_caller_closed_gate_tests.rs"]
mod closed_gate;

async fn legacy(suffix: &str) -> (Client, String, MongoSandboxStore, Agent) {
    let (client, name, store, agent, _) =
        super::super::admission_contexts::support::active(suffix).await;
    (
        client,
        name,
        store.with_segment_adoption_enabled(true),
        agent,
    )
}

async fn gate(client: &Client, name: &str, agent: &Agent, enabled: bool, selected: bool) {
    writer_control::execute(
        client,
        &client.database(name),
        writer_gate::GateName::Segments,
        writer_control::Action::Replace {
            expected_revision: 0,
            policy: writer_gate::Policy {
                enabled,
                agents: vec![if selected {
                    agent.id.to_string()
                } else {
                    "foreign-cohort".into()
                }],
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
}

async fn raw(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
) -> BTreeMap<String, Vec<RawDocumentBuf>> {
    let database = store.client.database(&store.agents.namespace().db);
    let mut rows = BTreeMap::new();
    for name in [
        "agents",
        "agent_log",
        "sandboxes",
        "agent_commands",
        "agent_command_orders",
        "model_budget_reservations",
        "agent_suggestions",
    ]
    .into_iter()
    .chain(segments::COLLECTIONS.iter().copied())
    {
        rows.insert(
            name.to_owned(),
            database
                .collection::<RawDocumentBuf>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .session(&mut *session)
                .await
                .unwrap()
                .stream(&mut *session)
                .try_collect()
                .await
                .unwrap(),
        );
    }
    rows
}

async fn snapshot_raw(store: &MongoSandboxStore) -> BTreeMap<String, Vec<RawDocumentBuf>> {
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let result = raw(store, &mut session).await;
    session.abort_transaction().await.unwrap();
    result
}

async fn version(store: &MongoSandboxStore, agent: &Agent) -> i64 {
    store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .get_i64("document_schema_version")
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_closed_absent_and_other_cohort_match_legacy_bson_exactly() {
    for mode in 0..3 {
        let (client, name, store, current) = legacy("caller_closed").await;
        if mode != 0 {
            gate(&client, &name, &current, mode == 2, mode == 1).await;
        }
        let mutation = output(&current, "identical natural output");
        let mut session = client.start_session().await.unwrap();
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        let expected = scoped::apply(
            scoped::PublicationSource::Conversation,
            &store,
            &mutation,
            None,
            None,
            None,
            &mut scope,
        )
        .await
        .unwrap();
        assert!(expected.segmented.is_none());
        let expected_bytes = raw(&store, scope.release()).await;
        session.abort_transaction().await.unwrap();
        assert_eq!(
            store
                .commit_agent_conversation(mutation.clone())
                .await
                .unwrap(),
            expected.logs
        );
        assert_eq!(
            snapshot_raw(&store).await,
            expected_bytes,
            "gate mode {mode}"
        );
        assert_eq!(version(&store, &current).await, 1);
        let current = mutation.agent;
        let mutation = output(&current, "identical checkpoint output");
        let next = boundary(&mutation);
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        let (expected, _) = scoped::checkpoint::apply(
            &store,
            (&current, None),
            None,
            &next,
            Some(&mutation),
            &conversation::checkpoint::Authority::Live,
            &mut scope,
        )
        .await
        .unwrap();
        let expected_bytes = raw(&store, scope.release()).await;
        session.abort_transaction().await.unwrap();
        assert_eq!(
            store
                .commit_agent_message_update(current, None, next, Some(mutation))
                .await
                .unwrap(),
            expected
        );
        assert_eq!(snapshot_raw(&store).await, expected_bytes);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_natural_conversation_adopts_without_an_extra_event() {
    let (client, name, store, current) = legacy("caller_adopts").await;
    gate(&client, &name, &current, true, true).await;
    let before = snapshot_raw(&store).await;
    let mutation = output(&current, "natural runtime output");
    let expected = conversation::conversation_logs(&mutation).unwrap();
    assert_eq!(
        store
            .commit_agent_conversation(mutation.clone())
            .await
            .unwrap(),
        expected
    );
    assert_eq!(version(&store, &current).await, 2);
    let after = snapshot_raw(&store).await;
    assert_eq!(
        &after["agent_log"][..before["agent_log"].len()],
        before["agent_log"]
    );
    assert_eq!(after["agent_log"].len(), before["agent_log"].len() + 1);
    assert_eq!(after[segments::PUBLICATIONS].len(), 1);
    assert_eq!(after[segments::writer_admission::COLLECTION].len(), 1);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .agent_log(&current.id, current.conversation.last_seq, 10)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        reopened.get_agent(&current.id).await.unwrap(),
        Some(mutation.agent)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_natural_checkpoint_adopts_and_replays_exactly() {
    let (client, name, store, current) = legacy("caller_checkpoint").await;
    gate(&client, &name, &current, true, true).await;
    let mutation = output(&current, "checkpoint boundary");
    let next = boundary(&mutation);
    assert_eq!(
        store
            .commit_agent_message_update(
                current.clone(),
                None,
                next.clone(),
                Some(mutation.clone())
            )
            .await
            .unwrap(),
        next.outcome
    );
    assert_eq!(version(&store, &current).await, 2);
    let after = snapshot_raw(&store).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .commit_agent_message_update(current, None, next.clone(), Some(mutation))
            .await
            .unwrap(),
        next.outcome
    );
    assert_eq!(snapshot_raw(&reopened).await, after);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_refuses_foreign_mutation_and_checkpoint_before_admission() {
    let (client, name, store, current) = legacy("caller_refusal").await;
    gate(&client, &name, &current, true, true).await;
    let before = snapshot_raw(&store).await;
    let mut mutation = output(&current, "original");
    mutation.agent.worktree_id = layer2_core::ids::WorktreeId("foreign".into());
    assert!(matches!(
        store.commit_agent_conversation(mutation).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot_raw(&store).await, before);
    let mutation = output(&current, "original");
    let mut checkpoint = boundary(&mutation);
    checkpoint.outcome.appended[0].kind = AgentLogKind::AgentMessage {
        content: "foreign".into(),
    };
    assert!(matches!(
        store
            .commit_agent_message_update(current, None, checkpoint, Some(mutation))
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot_raw(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_abort_preserves_source_and_exact_accepted_work_survives_close() {
    let (client, name, store, current) = legacy("caller_abort").await;
    gate(&client, &name, &current, true, true).await;
    let mutation = output(&current, "original");
    let prepared = scoped::adoption::prepare(&store, &mutation, None)
        .await
        .unwrap()
        .unwrap();
    let before = snapshot_raw(&store).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = scoped::apply(
        scoped::PublicationSource::Conversation,
        &store,
        &mutation,
        None,
        None,
        Some(&prepared),
        &mut scope,
    )
    .await
    .unwrap();
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(snapshot_raw(&store).await, before);
    assert!(!conversation::reconcile_conversation(&store, &receipt)
        .await
        .unwrap());
    writer_control::execute(
        &client,
        &client.database(&name),
        writer_gate::GateName::Segments,
        writer_control::Action::Close {
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap(),
        receipt.logs
    );
    assert!(conversation::reconcile_conversation(&store, &receipt)
        .await
        .unwrap());
    assert_eq!(version(&store, &current).await, 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_staged_source_conflict_cannot_adopt_or_replan() {
    let (client, name, store, current) = legacy("caller_conflict").await;
    gate(&client, &name, &current, true, true).await;
    let mutation = output(&current, "original");
    let prepared = scoped::adoption::prepare(&store, &mutation, None)
        .await
        .unwrap()
        .unwrap();
    let competing = output(&current, "competing old writer");
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    scoped::apply(
        scoped::PublicationSource::Conversation,
        &store,
        &competing,
        None,
        None,
        None,
        &mut scope,
    )
    .await
    .unwrap();
    scope.release().commit_transaction().await.unwrap();
    let before = snapshot_raw(&store).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(scoped::apply(
        scoped::PublicationSource::Conversation,
        &store,
        &mutation,
        None,
        None,
        Some(&prepared),
        &mut scope
    )
    .await
    .is_err());
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(snapshot_raw(&store).await, before);
    assert!(matches!(
        store.commit_agent_conversation(mutation).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot_raw(&store).await, before);
    assert_eq!(version(&store, &current).await, 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_caller_reserved_command_naturally_adopts_then_publishes_and_settles() {
    let (client, name) = database("caller_command").await;
    segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_segment_adoption_enabled(true);
    let ready = super::super::worker_discovery::finish_legacy_connection(
        &store,
        super::super::retained_assessment::ready_source(&store, "caller_command").await,
    )
    .await;
    gate(&client, &name, &ready, true, true).await;
    let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    let command = record(
        store
            .reserve_command(live_request(&ready, &parent, "caller-command"))
            .await
            .unwrap(),
    );
    assert_eq!(
        version(&store, &ready).await,
        1,
        "reservation alone does not adopt"
    );
    let claimed = store
        .claim_next_command(claim_request(
            &ready,
            "caller-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    let mutation = message_mutation(&ready, &parent, &claim);
    assert_eq!(
        store
            .commit_agent_conversation(mutation.clone())
            .await
            .unwrap(),
        conversation::conversation_logs(&mutation).unwrap()
    );
    assert_eq!(version(&store, &ready).await, 2);
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    recorder
        .record_update(
            &ready.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("natural answer"),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let result = recorder
        .record_update(
            &ready.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(result.turn_closed);
    let settled = store
        .reconcile_command(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(settled.status, AgentResponseStatus::Completed);
    assert!(settled.work.lease.is_none());
    client.database(&name).drop().await.unwrap();
}
