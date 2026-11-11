use super::capture_contract::{input, proposed};
use super::*;
use crate::store::{
    agent_log_segments::{
        capture, writer_admission as admission, writer_commit, writer_gate as gate,
        writer_reconcile,
    },
    sandbox_mongo::start_transaction,
};
use layer2_core::sandbox::AgentLogKind;

async fn admitted_then_closed(f: &Fixture, operation: &PublicationIdentity) -> Admission {
    let db = f.client.database(&f.store.agents.namespace().db);
    let mut session = transaction(f).await;
    start_transaction(&mut session).await.unwrap();
    succeeded(
        gate::replace(
            &db,
            gate::GateName::Segments,
            0,
            gate::Policy {
                enabled: true,
                agents: vec![f.agent.id.to_string()],
                ..Default::default()
            },
            &mut session,
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    start_transaction(&mut session).await.unwrap();
    let accepted = succeeded(
        admission::admit(
            &db,
            &f.agent,
            operation,
            admission::AdmissionKind::Rollover,
            NOW,
            &mut session,
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    start_transaction(&mut session).await.unwrap();
    succeeded(
        gate::replace(
            &db,
            gate::GateName::Segments,
            1,
            gate::Policy::default(),
            &mut session,
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    accepted
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn real_capture_rollover_requires_finite_admission_and_survives_gate_close() {
    let f = fixture(10_000).await;
    let (next, logs, operation) = proposed(&f);
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut scope).await);
    assert!(writer_commit::prepare(
        &f.store,
        &mut scope,
        &captured,
        input(&f, &next, &logs, &operation),
        None,
        None
    )
    .await
    .is_err());
    scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;

    let accepted = admitted_then_closed(&f, &operation).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut scope).await);
    let plan = succeeded(
        writer_commit::prepare(
            &f.store,
            &mut scope,
            &captured,
            input(&f, &next, &logs, &operation),
            None,
            Some(&accepted),
        )
        .await,
    );
    let receipt = succeeded(
        writer_commit::persist(
            &f.store,
            &mut scope,
            plan,
            AggregateUpdate {
                filter: doc! {"_id":f.agent.id.as_str()},
                fields: agent_replacement_fields(&next).unwrap(),
            },
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    assert_eq!(
        (receipt.active_ordinal, receipt.first_seq, receipt.last_seq),
        (1, 10_001, 10_001)
    );
    assert_eq!(
        collection(&f.store, SEGMENTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.store.agent_log.count_documents(doc! {}).await.unwrap(),
        10_001
    );
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    assert!(matches!(
        writer_reconcile::reconcile(
            &f.store,
            &operation,
            &next,
            &logs,
            &receipt.result_digest,
            &mut snapshot
        )
        .await
        .unwrap(),
        writer_reconcile::Outcome::Recorded(_)
    ));
    drop(snapshot);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn same_source_competing_captured_publications_commit_exactly_once() {
    let f = fixture(2).await;
    let (next, logs, first_operation) = proposed(&f);
    let mut second_operation = first_operation.clone();
    second_operation.key = "fixture-competing-publication".into();
    let mut session_a = transaction(&f).await;
    let mut session_b = transaction(&f).await;
    let mut first = TransactionScope::start(&mut session_a).await.unwrap();
    let mut second = TransactionScope::start(&mut session_b).await.unwrap();
    let captured_a = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut first).await);
    let captured_b = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut second).await);
    let plan_a = succeeded(
        writer_commit::prepare(
            &f.store,
            &mut first,
            &captured_a,
            input(&f, &next, &logs, &first_operation),
            None,
            None,
        )
        .await,
    );
    let plan_b = succeeded(
        writer_commit::prepare(
            &f.store,
            &mut second,
            &captured_b,
            input(&f, &next, &logs, &second_operation),
            None,
            None,
        )
        .await,
    );
    succeeded(
        writer_commit::persist(
            &f.store,
            &mut first,
            plan_a,
            AggregateUpdate {
                filter: doc! {"_id":f.agent.id.as_str()},
                fields: agent_replacement_fields(&next).unwrap(),
            },
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(first.release()).await,
        CommitOutcome::Committed
    ));
    assert!(writer_commit::persist(
        &f.store,
        &mut second,
        plan_b,
        AggregateUpdate {
            filter: doc! {"_id":f.agent.id.as_str()},
            fields: agent_replacement_fields(&next).unwrap()
        }
    )
    .await
    .is_err());
    let _ = second.release().abort_transaction().await;
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_append_tripwire_refuses_segmented_agent_without_changing_source() {
    let f = fixture(2).await;
    let mut session = transaction(&f).await;
    start_transaction(&mut session).await.unwrap();
    assert!(crate::store::agent_log::append_in_transaction(
        &f.store,
        &mut session,
        &f.agent.id,
        NOW.into(),
        AgentLogKind::ConversationGap { frames: 1 }
    )
    .await
    .is_err());
    session.abort_transaction().await.unwrap();
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn different_agents_publish_in_one_database_without_sharing_a_head_fence() {
    let a = fixture(2).await;
    let b = named_fixture(
        2,
        Some(&a.store.agents.namespace().db),
        "agt_independent_fixture",
    )
    .await;
    let (next_a, logs_a, op_a) = proposed(&a);
    let (next_b, logs_b, op_b) = proposed(&b);
    let mut session_a = transaction(&a).await;
    let mut session_b = transaction(&b).await;
    let mut scope_a = TransactionScope::start(&mut session_a).await.unwrap();
    let mut scope_b = TransactionScope::start(&mut session_b).await.unwrap();
    let source_a = succeeded(capture::capture_transaction(&a.store, &a.agent, &mut scope_a).await);
    let source_b = succeeded(capture::capture_transaction(&b.store, &b.agent, &mut scope_b).await);
    let plan_a = succeeded(
        writer_commit::prepare(
            &a.store,
            &mut scope_a,
            &source_a,
            input(&a, &next_a, &logs_a, &op_a),
            None,
            None,
        )
        .await,
    );
    let plan_b = succeeded(
        writer_commit::prepare(
            &b.store,
            &mut scope_b,
            &source_b,
            input(&b, &next_b, &logs_b, &op_b),
            None,
            None,
        )
        .await,
    );
    succeeded(
        writer_commit::persist(
            &a.store,
            &mut scope_a,
            plan_a,
            AggregateUpdate {
                filter: doc! {"_id":a.agent.id.as_str()},
                fields: agent_replacement_fields(&next_a).unwrap(),
            },
        )
        .await,
    );
    succeeded(
        writer_commit::persist(
            &b.store,
            &mut scope_b,
            plan_b,
            AggregateUpdate {
                filter: doc! {"_id":b.agent.id.as_str()},
                fields: agent_replacement_fields(&next_b).unwrap(),
            },
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(scope_a.release()).await,
        CommitOutcome::Committed
    ));
    assert!(matches!(
        commit_transaction(scope_b.release()).await,
        CommitOutcome::Committed
    ));
    assert_eq!(a.store.agent_log.count_documents(doc! {}).await.unwrap(), 6);
    assert_eq!(
        collection(&a.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    // Both fixtures share this one disposable database; remove it exactly once.
    drop(b);
    a.destroy().await;
}
