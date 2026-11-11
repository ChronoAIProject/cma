//! Public keyed boundary keeps an unresolved attempt across a fresh store.
use super::*;
use crate::store::agent_log_segments::publication;
use layer2_core::sandbox::{
    store::{AgentLogPublicationIdentity, AgentLogStore},
    AgentLogKind,
};
use mongodb::{options::ClientOptions, Client};

pub(super) async fn isolated(f: &mut Fixture) -> (Client, String) {
    let admin = f.client.clone();
    let name = format!("keyed-restart-{}", uuid::Uuid::new_v4().simple());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(name.clone());
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    (admin, name)
}

fn identity(kind: &AgentLogKind) -> AgentLogPublicationIdentity {
    AgentLogPublicationIdentity {
        key: "restart-original".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    }
}

async fn pending(f: &Fixture) -> Option<mongodb::bson::Bson> {
    f.store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .get("keyed_publication_attempt_v1")
        .cloned()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_without_receipt_survives_restart_and_later_head_without_replan() {
    let mut f = fixture(2).await;
    let (admin, app) = isolated(&mut f).await;
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let identity = identity(&kind);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"alwaysOn",
            "data":{"failCommands":["commitTransaction"],"appName":app,
            "errorCode":50_i32,"errorLabels":["UnknownTransactionCommitResult"]}
        })
        .await
        .unwrap();
    let result = f
        .store
        .append_agent_log_keyed(&f.agent.id, &identity, kind.clone())
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    let original = pending(&f)
        .await
        .expect("unresolved claim must survive failed acknowledgement");
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    // An independent fenced domain publication can advance the Agent. It must
    // preserve the unresolved keyed fence, even when its own receipt commits.
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let prepared = f.prepared(&scope);
    let aggregate = update(&prepared);
    succeeded(persist(&f.store, &mut scope, prepared, aggregate).await);
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    assert_eq!(pending(&f).await, Some(original.clone()));
    let restarted = MongoSandboxStore::open(&admin, &f.store.agents.namespace().db)
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .append_agent_log_keyed(&f.agent.id, &identity, kind.clone())
            .await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    let mut foreign = identity.clone();
    foreign.fingerprint[0] ^= 1;
    assert!(restarted
        .append_agent_log_keyed(&f.agent.id, &foreign, kind)
        .await
        .is_err());
    assert_eq!(pending(&f).await, Some(original));
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
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn positive_known_abort_releases_claim_and_retries_original_request() {
    let mut f = fixture(2).await;
    let (admin, app) = isolated(&mut f).await;
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let identity = identity(&kind);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":1_i32},
        "data":{"failCommands":["insert"],"appName":app,
        "errorCode":112_i32,"errorLabels":["TransientTransactionError"]}
        })
        .await
        .unwrap();
    let result = f
        .store
        .append_agent_log_keyed(&f.agent.id, &identity, kind)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result.unwrap().seq, 3);
    assert_eq!(pending(&f).await, None);
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
async fn publication_refusal_aborts_claim_without_any_result() {
    let f = fixture(2).await;
    let kind = AgentLogKind::ConversationOpened {
        conversation_id: "second-native".into(),
        runtime_generation: 1,
    };
    assert!(f
        .store
        .append_agent_log_keyed(&f.agent.id, &identity(&kind), kind)
        .await
        .is_err());
    assert_eq!(pending(&f).await, None);
    unchanged(&f).await;
    f.destroy().await;
}
