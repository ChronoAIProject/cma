use super::*;
use crate::store::{
    agent_admission_records::ReusedReceipt, agent_log_segments::transaction_scope::TransactionScope,
};
use futures_util::TryStreamExt;

const LATER: &str = "2026-09-05T00:01:00Z";

fn fresh_request(request: &AgentAdmission) -> AgentAdmission {
    let mut retry = request.clone();
    retry.agent_id = AgentId::generate();
    retry.worktree_id = WorktreeId::generate();
    retry.event_at = LATER.into();
    if let Some(message) = &request.first_message {
        retry.first_message = Some(
            layer2_core::sandbox::FirstMessageRecord::new(
                message.origin_key.clone(),
                message.text.clone(),
                LATER,
            )
            .unwrap(),
        );
    }
    assert_eq!(retry.fingerprint().unwrap(), request.fingerprint().unwrap());
    retry
}

async fn retained_rows(store: &MongoSandboxStore) -> Vec<(&'static str, Vec<Document>)> {
    let database = store.client.database(&store.agents.namespace().db);
    let mut rows = Vec::new();
    for name in [
        "agents",
        "worktrees",
        "workspaces",
        "sandboxes",
        "agent_log",
        "agent_log_heads",
        "agent_log_segments",
        "agent_log_projections",
        "agent_log_publications",
        "agent_reuse_bindings",
        "agent_commands",
        "trigger_first_message_bindings",
    ] {
        let documents = database
            .collection::<Document>(name)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        rows.push((name, documents));
    }
    rows
}

async fn replay_with_fresh_time(suffix: &str, first_message: bool) {
    let (client, database, store, mut request, created) = adopted(suffix).await;
    if !first_message {
        request.first_message = None;
    }
    let accepted = store
        .admit_agent_reusing_profile(request.clone())
        .await
        .unwrap();
    assert_eq!(accepted.creation().agent.id, created.agent.id);
    for later_append in [false, true] {
        if later_append {
            append(&store, &created.agent.id, "append-after-accepted-reuse").await;
        }
        let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
        let before = retained_rows(&reopened).await;
        let agent = reopened
            .get_agent(&created.agent.id)
            .await
            .unwrap()
            .unwrap();
        let result = reopened
            .admit_agent_reusing_profile(fresh_request(&request))
            .await;
        assert_eq!(retained_rows(&reopened).await, before);
        let replay = result.expect("same identity with a fresh service time must replay");
        assert!(matches!(replay, AgentAdmissionResult::Existing(_)));
        assert_eq!(replay.creation().agent, agent);
        assert_eq!(replay.creation().initial_log, created.initial_log);
        assert_eq!(replay.creation().worktree, created.worktree);
        assert_eq!(
            replay.creation().agent.conversation.first_message,
            accepted.creation().agent.conversation.first_message
        );
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn fresh_first_message_retry_replays_after_restart_and_later_append() {
    Box::pin(replay_with_fresh_time("fresh_reuse_message", true)).await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn fresh_metadata_retry_replays_after_restart_and_later_append() {
    Box::pin(replay_with_fresh_time("fresh_reuse_metadata", false)).await;
}

async fn require_refusal(store: &MongoSandboxStore, request: AgentAdmission) {
    let before = retained_rows(store).await;
    assert!(store.admit_agent_reusing_profile(request).await.is_err());
    assert_eq!(retained_rows(store).await, before);
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn fresh_retry_refuses_changed_identity_or_retained_timestamp_evidence() {
    let (client, database, store, request, created) = adopted("fresh_reuse_corruption").await;
    store
        .admit_agent_reusing_profile(request.clone())
        .await
        .unwrap();
    append(&store, &created.agent.id, "append-before-proof-corruption").await;
    let id = crate::store::agent_admission_records::binding_id(&request).unwrap();
    let original = store
        .agent_reuse_bindings
        .find_one(doc! {"_id":&id})
        .await
        .unwrap()
        .unwrap();
    let publication = original.get_document("log_publication").unwrap().clone();
    let publications = client
        .database(&database)
        .collection::<Document>("agent_log_publications");
    let publication_id = publication.get_str("_id").unwrap();
    let mut changed = fresh_request(&request);
    changed.title = Some("Different admission identity".into());
    require_refusal(&store, changed).await;
    // Mutating either copy must fail their equality proof. Mutating both must
    // still fail against the original retained event's timestamp and digest.
    for (binding_time, publication_time) in [(true, false), (false, true), (true, true)] {
        if binding_time {
            store
                .agent_reuse_bindings
                .update_one(
                    doc! {"_id":&id},
                    doc! {"$set":{"log_publication.appended_at":LATER}},
                )
                .await
                .unwrap();
        }
        if publication_time {
            publications
                .update_one(
                    doc! {"_id":publication_id},
                    doc! {"$set":{"appended_at":LATER}},
                )
                .await
                .unwrap();
        }
        require_refusal(&store, fresh_request(&request)).await;
        store
            .agent_reuse_bindings
            .replace_one(doc! {"_id":&id}, original.clone())
            .await
            .unwrap();
        publications
            .replace_one(doc! {"_id":publication_id}, publication.clone())
            .await
            .unwrap();
    }
    publications
        .delete_one(doc! {"_id":publication_id})
        .await
        .unwrap();
    require_refusal(&store, fresh_request(&request)).await;
    publications.insert_one(publication).await.unwrap();
    let replay = store
        .admit_agent_reusing_profile(fresh_request(&request))
        .await;
    client.database(&database).drop().await.unwrap();
    assert!(
        replay.is_ok(),
        "restored original evidence must allow replay"
    );
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn ambiguous_reuse_keeps_the_original_attempt_timestamp_strict() {
    let (client, database, store, request, created) = adopted("reuse_attempt_time").await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = match Box::pin(crate::store::agent_log_segments::admission::apply(
        &store, &request, true, None, &mut scope,
    ))
    .await
    {
        Ok(AdmissionReceipt::Reused(receipt)) => receipt,
        _ => panic!("expected one original segmented reuse publication"),
    };
    scope.release().commit_transaction().await.unwrap();
    append(&store, &created.agent.id, "append-after-ambiguous-reuse").await;
    store
        .agents
        .find_one(doc! {"_id":created.agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    let at = session.operation_time();
    let before = retained_rows(&store).await;
    let mut changed = receipt.recorded.clone();
    changed.operation.appended_at = LATER.into();
    let result = crate::store::agent_admission::reuse_reconciliation::after_unknown(
        &store,
        &request,
        Box::new(ReusedReceipt {
            creation: receipt.creation.clone(),
            recorded: changed,
        }),
        at,
    )
    .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    let expected = AgentAdmissionResult::Existing(receipt.creation.clone());
    assert_eq!(
        crate::store::agent_admission::reuse_reconciliation::after_unknown(
            &store, &request, receipt, at,
        )
        .await
        .unwrap(),
        expected
    );
    assert_eq!(retained_rows(&store).await, before);
    client.database(&database).drop().await.unwrap();
}
