//! Production proof path: no fixture constructor for CapturedLog or plans.
use super::*;
use crate::store::agent_log_segments::{capture, publication, writer_commit};
use layer2_core::sandbox::AgentLogKind;
use mongodb::Collection;

pub(super) fn proposed(f: &Fixture) -> (Agent, Vec<AgentLogEntry>, PublicationIdentity) {
    let logs = vec![AgentLogEntry {
        agent_id: f.agent.id.clone(),
        seq: f.agent.conversation.last_seq + 1,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationGap { frames: 1 },
    }];
    let mut next = f.agent.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    let operation = PublicationIdentity {
        key: "fixture-captured-publication".into(),
        fingerprint: publication::request_digest(&logs).unwrap(),
        appended_at: NOW.into(),
    };
    (next, logs, operation)
}

pub(super) fn input<'a>(
    f: &'a Fixture,
    next: &'a Agent,
    logs: &'a [AgentLogEntry],
    operation: &'a PublicationIdentity,
) -> PublicationInput<'a> {
    PublicationInput {
        operation,
        current: &f.agent,
        next,
        logs,
        capacity: AppendCapacity::Normal,
        result_digest: publication::result_digest(next).unwrap(),
    }
}

async fn domain_fixture(f: &Fixture) -> Collection<Document> {
    let database = f.client.database(&f.store.agents.namespace().db);
    let name = "writer_contract_domain_fixture";
    database.create_collection(name).await.unwrap();
    database.collection(name)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn captured_publication_commits_and_aborts_with_caller_domain_work() {
    for commit in [false, true] {
        let f = fixture(2).await;
        let domain = domain_fixture(&f).await;
        let (next, logs, operation) = proposed(&f);
        let mut session = transaction(&f).await;
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        let captured =
            succeeded(capture::capture_transaction(&f.store, &f.agent, &mut scope).await);
        let plan = succeeded(
            writer_commit::prepare(
                &f.store,
                &mut scope,
                &captured,
                input(&f, &next, &logs, &operation),
                None,
                None,
            )
            .await,
        );
        scope
            .insert_one(&domain, doc! {"_id":"original-domain-operation"})
            .await
            .unwrap();
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
        if commit {
            assert!(matches!(
                commit_transaction(scope.release()).await,
                CommitOutcome::Committed
            ));
            let row = f
                .store
                .agents
                .find_one(doc! {"_id":f.agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(crate::store::agents::read_agent(&row).unwrap(), next);
            assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 3);
            assert!(collection(&f.store, PUBLICATIONS)
                .find_one(doc! {"_id":receipt.id})
                .await
                .unwrap()
                .is_some());
            assert_eq!(domain.count_documents(doc! {}).await.unwrap(), 1);
        } else {
            scope.release().abort_transaction().await.unwrap();
            unchanged(&f).await;
            assert_eq!(
                collection(&f.store, PUBLICATIONS)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(domain.count_documents(doc! {}).await.unwrap(), 0);
        }
        f.destroy().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn captured_source_and_prepared_plan_cannot_cross_transaction_restarts() {
    let f = fixture(2).await;
    let (next, logs, operation) = proposed(&f);
    let mut session = transaction(&f).await;
    let mut first = TransactionScope::start(&mut session).await.unwrap();
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut first).await);
    let plan = succeeded(
        writer_commit::prepare(
            &f.store,
            &mut first,
            &captured,
            input(&f, &next, &logs, &operation),
            None,
            None,
        )
        .await,
    );
    first.release().abort_transaction().await.unwrap();
    let mut later = TransactionScope::start(&mut session).await.unwrap();
    assert!(writer_commit::prepare(
        &f.store,
        &mut later,
        &captured,
        input(&f, &next, &logs, &operation),
        None,
        None
    )
    .await
    .is_err());
    assert!(writer_commit::persist(
        &f.store,
        &mut later,
        plan,
        AggregateUpdate {
            filter: doc! {"_id":f.agent.id.as_str()},
            fields: agent_replacement_fields(&next).unwrap()
        }
    )
    .await
    .is_err());
    later.release().abort_transaction().await.unwrap();
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
async fn snapshot_source_cannot_be_promoted_into_publication_authority() {
    let f = fixture(2).await;
    let (next, logs, operation) = proposed(&f);
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    let captured = succeeded(
        capture::capture(
            &f.store,
            &f.agent,
            &mut snapshot,
            capture::CaptureMode::Snapshot,
        )
        .await,
    );
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
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
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    drop(snapshot);
    f.destroy().await;
}
