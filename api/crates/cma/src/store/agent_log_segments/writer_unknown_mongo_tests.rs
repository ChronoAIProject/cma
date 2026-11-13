use super::capture_contract::{input, proposed};
use super::*;
use crate::store::agent_log_segments::{
    capture, publication, writer_commit,
    writer_reconcile::{self, Outcome},
};
use mongodb::{options::ClientOptions, Client};

async fn apply(
    f: &Fixture,
    scope: &mut TransactionScope<'_>,
    next: &Agent,
    logs: &[AgentLogEntry],
    operation: &PublicationIdentity,
) -> Publication {
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, scope).await);
    let plan = succeeded(
        writer_commit::prepare(
            &f.store,
            scope,
            &captured,
            input(f, next, logs, operation),
            None,
            None,
        )
        .await,
    );
    succeeded(
        writer_commit::persist(
            &f.store,
            scope,
            plan,
            AggregateUpdate {
                filter: doc! {"_id":f.agent.id.as_str()},
                fields: agent_replacement_fields(next).unwrap(),
            },
        )
        .await,
    )
}

async fn reconcile(
    f: &Fixture,
    next: &Agent,
    logs: &[AgentLogEntry],
    operation: &PublicationIdentity,
    at: Option<mongodb::bson::Timestamp>,
) -> Outcome {
    let mut start = f.client.start_session().snapshot(true);
    if let Some(at) = at {
        start = start.snapshot_time(at);
    }
    let mut snapshot = start.await.unwrap();
    writer_reconcile::reconcile(
        &f.store,
        operation,
        next,
        logs,
        &publication::result_digest(next).unwrap(),
        &mut snapshot,
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_commit_is_reconciled_from_original_receipt_after_later_head_advance() {
    let mut f = fixture(2).await;
    let admin = f.client.clone();
    let application = format!("writer-unknown-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(application.clone());
    let client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    f.client = client;
    let (next, logs, operation) = proposed(&f);
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = apply(&f, &mut scope, &next, &logs, &operation).await;
    // An uncommitted publication is Unknown, never proof of an abort.
    assert!(matches!(
        reconcile(&f, &next, &logs, &operation, None).await,
        Outcome::Unknown
    ));
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":application,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected writer commit ambiguity"}}
    }).await.unwrap();
    let outcome = commit_transaction(scope.release()).await;
    let disabled = admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await;
    disabled.expect("disable the isolated commit failpoint before assertions");
    assert!(matches!(outcome, CommitOutcome::Unknown(_)));
    assert!(matches!(
        reconcile(&f, &next, &logs, &operation, session.operation_time()).await,
        Outcome::Recorded(_)
    ));

    f.agent = next.clone();
    let (later, later_logs, mut later_operation) = proposed(&f);
    later_operation.key = "fixture-later-head".into();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    apply(&f, &mut scope, &later, &later_logs, &later_operation).await;
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    let Outcome::Recorded(original) =
        reconcile(&f, &next, &logs, &operation, session.operation_time()).await
    else {
        panic!("original outcome must survive later publications")
    };
    assert_eq!(original.id, receipt.id);
    assert_eq!(original.result_digest, receipt.result_digest);
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 4);
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );

    let mut conflicting = operation.clone();
    conflicting.fingerprint = publication::request_digest(&"different request").unwrap();
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    assert!(writer_reconcile::reconcile(
        &f.store,
        &conflicting,
        &next,
        &logs,
        &receipt.result_digest,
        &mut snapshot
    )
    .await
    .is_err());
    drop(snapshot);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn known_abort_can_retry_original_identity_without_partial_evidence() {
    let f = fixture(2).await;
    let (next, logs, operation) = proposed(&f);
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let original = apply(&f, &mut scope, &next, &logs, &operation).await;
    scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;
    assert!(matches!(
        reconcile(&f, &next, &logs, &operation, None).await,
        Outcome::Unknown
    ));
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let retried = apply(&f, &mut scope, &next, &logs, &operation).await;
    assert_eq!(
        bson::serialize_to_document(&original).unwrap(),
        bson::serialize_to_document(&retried).unwrap()
    );
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    assert!(matches!(
        reconcile(&f, &next, &logs, &operation, session.operation_time()).await,
        Outcome::Recorded(_)
    ));
    f.destroy().await;
}
