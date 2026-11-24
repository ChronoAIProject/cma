use super::*;
use crate::store::sandbox_mongo_integration_tests::native_home_fixture::native_home;
use crate::store::sandbox_operator_mongo_integration_tests::{agent, database, sandbox};
use layer2_core::sandbox::store::{AgentLogStore, AgentStore, SandboxStore};
use layer2_core::sandbox::{AgentLogKind, AgentStatus};

const NOW: &str = "2026-09-19T00:00:00Z";
const CUTOFF: &str = "2026-09-17T00:00:00Z";

fn fixture(id: &str) -> (Sandbox, ReviewedRetirementMember) {
    let mut sandbox = sandbox(id, "test-owner", "reviewed", SandboxStatus::Active, "old");
    let home = native_home(&sandbox.id);
    let runtime = sandbox.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    runtime.runtime_version = home
        .native_pool
        .as_ref()
        .unwrap()
        .request
        .pool
        .spec
        .image
        .uri
        .clone();
    let member = ReviewedRetirementMember {
        batch_sha256: "a".repeat(64),
        member_sha256: "b".repeat(64),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        created_at: sandbox.created_at.clone(),
        revision: sandbox.revision,
        status: sandbox.status,
        last_activity_at: sandbox.last_activity_at.clone(),
        idle_before: CUTOFF.into(),
        runtime: runtime.clone(),
        home: Box::new(home),
    };
    sandbox.lifecycle.home = Some(member.home.clone());
    (sandbox, member)
}

async fn seed(store: &MongoSandboxStore, id: &str) -> (Sandbox, ReviewedRetirementMember) {
    let (sandbox, member) = fixture(id);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    (sandbox, member)
}

#[test]
#[ignore = "runs the operator CLI wire compatibility check with Python 3"]
fn executor_matches_rust_member_and_receipt_wire_bytes() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let (_, mut member) = fixture("sbx_receipt_wire");
    member.owner = layer2_core::SubjectId("synthetic-用户".into());
    assert!(member.validate(NOW));
    let receipt = ReviewedRetirementReceipt {
        batch_sha256: member.batch_sha256.clone(),
        member_sha256: member.member_sha256.clone(),
        guard_sha256: member.fingerprint().unwrap(),
        sandbox_id: member.sandbox_id.clone(),
        accepted_revision: member.revision + 1,
        event_sequence: 3,
    };
    let vector = serde_json::to_vec(&(member, receipt)).unwrap();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.github/scripts/sandbox-retirement-receipt-vector.py"
    );
    let mut child = Command::new("python3")
        .args(["-B", script])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let sent = child.stdin.take().unwrap().write_all(&vector);
    let status = child.wait().unwrap();
    sent.unwrap();
    assert!(status.success(), "the executor rejected the Rust receipt");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn persists_exact_batch_member_receipt_and_retirement_event() {
    let (client, name, store) = database("reviewed_retirement").await;
    let (original, member) = seed(&store, "sbx_reviewed").await;
    let accepted = store.admit_reviewed_retirement(&member, NOW).await.unwrap();
    let ReviewedRetirementOutcome::Accepted(receipt) = &accepted else {
        panic!("unexpected admission: {accepted:?}");
    };
    assert_eq!(receipt.accepted_revision, original.revision + 1);
    let independent = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        independent
            .reviewed_retirement_receipt(
                &member.sandbox_id,
                &member.batch_sha256,
                &member.member_sha256
            )
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    let events = independent
        .sandbox_events(&member.sandbox_id, 0, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, receipt.event_sequence);
    assert_eq!(
        independent
            .admit_reviewed_retirement(&member, NOW)
            .await
            .unwrap(),
        accepted
    );
    let mut altered = member.clone();
    altered.idle_before = "2026-09-16T00:00:00Z".into();
    assert!(altered.validate(NOW));
    assert_eq!(
        independent
            .admit_reviewed_retirement(&altered, NOW)
            .await
            .unwrap(),
        ReviewedRetirementOutcome::NotEligible
    );
    let mut other_batch = member.clone();
    other_batch.batch_sha256 = "c".repeat(64);
    assert_eq!(
        independent
            .admit_reviewed_retirement(&other_batch, NOW)
            .await
            .unwrap(),
        ReviewedRetirementOutcome::ObserveExisting
    );
    assert!(independent
        .reviewed_retirement_receipt(
            &member.sandbox_id,
            &other_batch.batch_sha256,
            &member.member_sha256
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        independent
            .sandbox_events(&member.sandbox_id, 0, 10)
            .await
            .unwrap(),
        events
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn child_publication_between_snapshot_and_retirement_prevents_admission() {
    let (client, name, store) = database("reviewed_retirement_race").await;
    let (original, member) = seed(&store, "sbx_reviewed_race").await;
    let mut child = agent(&original, "agt_reviewed_race", AgentStatus::Failed);
    store.create_agent(child.clone()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let sources = CapturedActivities::capture(&store, &member.sandbox_id, &mut scope)
        .await
        .unwrap();
    let prior = child.revision;
    child.revision += 1;
    child.updated_at = NOW.into();
    store.save_agent(prior, child).await.unwrap();
    let session = scope.release();
    let result = apply_after_snapshot(&store, &member, NOW, &sources, &mut *session).await;
    assert!(matches!(
        result,
        Err(ApplyError::Driver(_)) | Err(ApplyError::Domain(SandboxStoreError::Conflict))
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store.get_sandbox(&member.sandbox_id).await.unwrap(),
        Some(original)
    );
    assert!(store
        .sandbox_events(&member.sandbox_id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.admit_reviewed_retirement(&member, NOW).await.unwrap(),
        ReviewedRetirementOutcome::NotEligible
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn quiet_live_agent_cannot_hide_a_terminal_siblings_queued_command() {
    let (client, name, store) = database("reviewed_retirement_mixed").await;
    let (sandbox, member) = seed(&store, "sbx_reviewed_mixed").await;
    let ready = agent(&sandbox, "agt_reviewed_ready", AgentStatus::Ready);
    let failed = agent(&sandbox, "agt_reviewed_failed", AgentStatus::Failed);
    store.create_agent(ready.clone()).await.unwrap();
    store.create_agent(failed.clone()).await.unwrap();
    store
        .append_agent_log(
            &ready.id,
            ready.created_at.clone(),
            AgentLogKind::ConversationOpened {
                conversation_id: ready.conversation.conversation_id.unwrap(),
                runtime_generation: 1,
            },
        )
        .await
        .unwrap();
    store
        .agent_commands
        .insert_one(doc! {
            "_id": "terminal-queued",
            "document_schema_version": crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            "agent_id": failed.id.as_str(),
            "status": "queued",
        })
        .await
        .unwrap();
    assert_eq!(
        store.admit_reviewed_retirement(&member, NOW).await.unwrap(),
        ReviewedRetirementOutcome::NotEligible
    );
    assert_unadmitted(&store, &member, &sandbox).await;
    store
        .agent_commands
        .delete_one(doc! { "_id": "terminal-queued" })
        .await
        .unwrap();
    let admitted = store.admit_reviewed_retirement(&member, NOW).await.unwrap();
    assert!(
        matches!(admitted, ReviewedRetirementOutcome::Accepted(_)),
        "{admitted:?}"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn matching_request_and_record_cannot_replace_the_native_home_runtime_uid() {
    let (client, name, store) = database("reviewed_retirement_replacement").await;
    let (original, member) = seed(&store, "sbx_reviewed_replacement").await;
    assert!(member.validate(NOW));
    let mut changed = original.clone();
    changed.revision += 1;
    let replacement = changed.runtime.as_mut().unwrap();
    replacement.runtime_id = "agent-sandbox:member:replacement-uid".into();
    let mut request = member;
    request.revision = changed.revision;
    request.runtime = replacement.clone();
    store
        .save_sandbox(original.revision, changed.clone())
        .await
        .unwrap();
    assert!(request.matches(&changed));
    assert!(!request.validate(NOW));
    assert_eq!(
        store
            .admit_reviewed_retirement(&request, NOW)
            .await
            .unwrap(),
        ReviewedRetirementOutcome::NotEligible
    );
    assert_unadmitted(&store, &request, &changed).await;
    client.database(&name).drop().await.unwrap();
}

async fn assert_unadmitted(
    store: &MongoSandboxStore,
    member: &ReviewedRetirementMember,
    sandbox: &Sandbox,
) {
    assert_eq!(
        store
            .get_sandbox(&member.sandbox_id)
            .await
            .unwrap()
            .as_ref(),
        Some(sandbox)
    );
    assert!(store
        .sandbox_events(&member.sandbox_id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .reviewed_retirement_receipt(
            &member.sandbox_id,
            &member.batch_sha256,
            &member.member_sha256
        )
        .await
        .unwrap()
        .is_none());
}
