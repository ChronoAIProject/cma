//! The real caller's uncertain outcome settles only its original transaction.
use super::*;
use crate::store::agent_log::keyed::{settlement::after_unknown, Attempt};
use crate::store::agent_log_segments::{capture, publication, writer_commit};
use layer2_core::sandbox::{
    store::{AgentLogPublicationIdentity, AgentLogStore},
    AgentLogKind,
};
use mongodb::{options::ClientOptions, Client};

async fn attempted(
    f: &Fixture,
    scope: &mut TransactionScope<'_>,
) -> (PublicationIdentity, AgentLogEntry, Attempt) {
    let entry = AgentLogEntry {
        agent_id: f.agent.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationGap { frames: 1 },
    };
    let operation = PublicationIdentity {
        key: "original-keyed-attempt".into(),
        fingerprint: publication::request_digest(&entry.kind).unwrap(),
        appended_at: NOW.into(),
    };
    let mut next = f.agent.clone();
    next.conversation.last_seq = entry.seq;
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, scope).await);
    let plan = succeeded(
        writer_commit::prepare(
            &f.store,
            scope,
            &captured,
            PublicationInput {
                operation: &operation,
                current: &f.agent,
                next: &next,
                logs: std::slice::from_ref(&entry),
                capacity: AppendCapacity::Normal,
                result_digest: publication::result_digest(&entry).unwrap(),
            },
            None,
            None,
        )
        .await,
    );
    let publication = succeeded(
        persist(
            &f.store,
            scope,
            plan,
            AggregateUpdate {
                filter: doc! {"_id":f.agent.id.as_str()},
                fields: agent_replacement_fields(&next).unwrap(),
            },
        )
        .await,
    );
    (operation, entry, Attempt { next, publication })
}

async fn observed_time(f: &Fixture) -> mongodb::bson::Timestamp {
    let mut session = f.client.start_session().await.unwrap();
    f.store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    session.operation_time().unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn original_caller_receipt_accepts_commit_and_refuses_foreign_or_aborted_attempt() {
    let f = fixture(2).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let (operation, entry, original) = attempted(&f, &mut scope).await;
    scope.release().abort_transaction().await.unwrap();
    let at = observed_time(&f).await;
    assert!(matches!(
        after_unknown(&f.store, &operation, &entry, &original, Some(at)).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );

    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let (_, _, committed) = attempted(&f, &mut scope).await;
    assert_eq!(committed.publication, original.publication);
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    let at = observed_time(&f).await;
    assert_eq!(
        after_unknown(&f.store, &operation, &entry, &original, Some(at))
            .await
            .unwrap(),
        entry
    );
    // Every receipt field matters, including the exact originally proposed head.
    let mut foreign = original;
    foreign.publication.head_revision += 1;
    assert_eq!(
        after_unknown(&f.store, &operation, &entry, &foreign, Some(at)).await,
        Err(SandboxStoreError::Conflict)
    );
    assert!(matches!(
        after_unknown(&f.store, &operation, &entry, &committed, None).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    let mut wrong_result = entry.clone();
    wrong_result.kind = AgentLogKind::ConversationGap { frames: 2 };
    assert!(
        after_unknown(&f.store, &operation, &wrong_result, &committed, Some(at))
            .await
            .is_err()
    );
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 3);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn keyed_caller_settles_uncertain_commit_without_republishing() {
    let mut f = fixture(2).await;
    let admin = f.client.clone();
    let application = format!("keyed-caller-{}", uuid::Uuid::new_v4().simple());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(application.clone());
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let identity = AgentLogPublicationIdentity {
        key: "caller-uncertain".into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
        appended_at: NOW.into(),
    };
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":application,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected uncertainty"}}
    }).await.unwrap();
    let result = f
        .store
        .append_agent_log_keyed(&f.agent.id, &identity, kind.clone())
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let original = result.unwrap();
    assert_eq!(original.seq, 3);
    assert_eq!(
        f.store
            .append_agent_log_keyed(&f.agent.id, &identity, kind)
            .await
            .unwrap(),
        original
    );
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
