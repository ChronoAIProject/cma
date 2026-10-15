use super::*;
use crate::store::sandboxes::sandbox_replacement_fields;
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::sandbox::{Sandbox, SandboxWorkState, SandboxWorkStep};
use std::sync::Arc;

fn baseline() -> Vec<ServiceId> {
    vec![
        ServiceId::new("chrono-llm-public"),
        ServiceId::new("ornn-api"),
    ]
}
fn authority() -> AgentAuthorizationRevision {
    AgentAuthorizationRevision {
        authority_id: "personal:owner-scope".into(),
        policy_revision: 1,
        grant_revision: None,
    }
}
async fn put(store: &MongoSandboxStore, sandbox: &Sandbox) {
    sandbox.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(sandbox).unwrap()},
        )
        .await
        .unwrap();
}
async fn fixture() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
    Sandbox,
) {
    let (client, database, store, original) = super::super::tests::approved_fixture().await;
    store
        .fail_access_request(original.lease.as_ref().unwrap(), "fixture_complete")
        .await
        .unwrap();
    let mut sandbox = store
        .get_sandbox(&original.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    sandbox.credential.reaches = baseline();
    sandbox
        .credential
        .reaches
        .push(ServiceId::new("api-github"));
    sandbox.credential.reaches.sort();
    put(&store, &sandbox).await;
    (client, database, store, original, sandbox)
}
async fn save(
    store: &MongoSandboxStore,
    original: &AgentAccessRecord,
    desired: Vec<ServiceId>,
    expected: Vec<ServiceId>,
    key: &str,
) -> Result<AgentAccessRecord, AgentAccessError> {
    store
        .replace_agent_service_access(
            &original.requester.owner,
            &original.requester.agent_id,
            desired,
            expected,
            &baseline(),
            authority(),
            key,
        )
        .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_owner_save_removes_scope_hot_and_replays_after_scope_has_changed() {
    let (client, name, store, original, before) = fixture().await;
    let expected = before.credential.effective_reaches();
    let saved = save(
        &store,
        &original,
        baseline(),
        expected.clone(),
        "remove-github",
    )
    .await
    .unwrap();
    assert_eq!(saved.phase, AgentAccessPhase::Connecting);
    assert_eq!(saved.expected_scope_revision, Some(0));
    assert_eq!(saved.approved_services().unwrap(), baseline());
    assert_eq!(
        store.get_sandbox(&before.id).await.unwrap(),
        Some(before.clone())
    );
    let claimed = store
        .claim_access_request("owner-scope")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, saved.id);
    let applied = store
        .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(applied.phase, AgentAccessPhase::Updating);
    let after = store.get_sandbox(&before.id).await.unwrap().unwrap();
    assert_eq!(after.credential.effective_reaches(), baseline());
    assert_eq!(after.runtime, before.runtime);
    assert_eq!(after.lifecycle, before.lifecycle);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        save(
            &reopened,
            &original,
            baseline(),
            expected.clone(),
            "remove-github"
        )
        .await
        .unwrap(),
        applied
    );
    assert_eq!(
        save(
            &reopened,
            &original,
            expected.clone(),
            expected,
            "remove-github"
        )
        .await,
        Err(AgentAccessError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_owner_save_rejects_baseline_removal_stale_scope_duplicate_and_other_owner() {
    let (client, name, store, original, before) = fixture().await;
    let count = store
        .agent_access_requests
        .count_documents(doc! {})
        .await
        .unwrap();
    let expected = before.credential.effective_reaches();
    for desired in [
        vec![ServiceId::new("ornn-api")],
        vec![ServiceId::new("chrono-llm-public")],
        vec![
            ServiceId::new("chrono-llm-public"),
            ServiceId::new("ornn-api"),
            ServiceId::new("ornn-api"),
        ],
    ] {
        assert_eq!(
            save(&store, &original, desired, expected.clone(), "invalid").await,
            Err(AgentAccessError::Invalid)
        );
    }
    assert_eq!(
        save(&store, &original, baseline(), baseline(), "stale").await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(
        store
            .replace_agent_service_access(
                &SubjectId::new("foreign"),
                &original.requester.agent_id,
                baseline(),
                expected,
                &baseline(),
                authority(),
                "foreign"
            )
            .await,
        Err(AgentAccessError::NotFound)
    );
    assert_eq!(
        store
            .agent_access_requests
            .count_documents(doc! {})
            .await
            .unwrap(),
        count
    );
    assert_eq!(store.get_sandbox(&before.id).await.unwrap(), Some(before));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_competing_owner_saves_cannot_apply_stale_scope_after_the_winner() {
    let (client, name, store, original, before) = fixture().await;
    let expected = before.credential.effective_reaches();
    let first = save(
        &store,
        &original,
        baseline(),
        expected.clone(),
        "first-save",
    )
    .await
    .unwrap();
    let mut added = expected.clone();
    added.push(ServiceId::new("calendar"));
    let second = save(&store, &original, added, expected, "second-save")
        .await
        .unwrap();
    let claimed = store
        .claim_access_request("first-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, first.id);
    let applied = store
        .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
        .await
        .unwrap();
    store
        .fail_access_request(applied.lease.as_ref().unwrap(), "fixture_complete")
        .await
        .unwrap();
    let claimed = store
        .claim_access_request("second-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, second.id);
    assert_eq!(
        store
            .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
            .await,
        Err(AgentAccessError::Invalid)
    );
    assert_eq!(
        store
            .get_sandbox(&before.id)
            .await
            .unwrap()
            .unwrap()
            .credential
            .effective_reaches(),
        baseline()
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_owner_save_while_paused_records_consent_without_resuming_or_rotating() {
    let (client, name, store, original, before) = fixture().await;
    let mut paused = before.clone();
    paused
        .begin_pause(layer2_core::now_rfc3339(), None)
        .unwrap();
    paused.lifecycle.operation = None;
    paused.status = SandboxStatus::Paused;
    paused.lifecycle.suspended_runtime = paused.runtime.take().map(Box::new);
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    put(&store, &paused).await;
    let saved = save(
        &store,
        &original,
        baseline(),
        before.credential.effective_reaches(),
        "paused-save",
    )
    .await
    .unwrap();
    assert_eq!(saved.phase, AgentAccessPhase::Connecting);
    assert_eq!(saved.requester.runtime, before.runtime.unwrap());
    assert_eq!(store.get_sandbox(&paused.id).await.unwrap(), Some(paused));
    assert_eq!(saved.progress, AgentAccessProgress::default());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_notification_transition_requires_old_key_revoked_and_exact_scope() {
    let (client, name, store, original, before) = fixture().await;
    save(
        &store,
        &original,
        baseline(),
        before.credential.effective_reaches(),
        "notify-guards",
    )
    .await
    .unwrap();
    let claimed = store
        .claim_access_request("notify-guards")
        .await
        .unwrap()
        .unwrap();
    let applied = store
        .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
        .await
        .unwrap();
    let mut ready = store.get_sandbox(&before.id).await.unwrap().unwrap();
    ready.credential.key_generation += 1;
    let current = ready.credential.current.as_mut().unwrap();
    current.generation = ready.credential.key_generation;
    current.provider_key_id = "new-scope-key".into();
    let mut progress = applied.progress.clone();
    progress.delivered_credential = ready.credential.current.clone();
    progress.delivered_runtime = ready.runtime.clone();
    for case in 0..4 {
        let mut changed = ready.clone();
        match case {
            0 => changed
                .credential
                .queue_revoke("previous-scope-key".into(), layer2_core::now_rfc3339()),
            1 => changed
                .credential
                .service_scope
                .as_mut()
                .unwrap()
                .services
                .push(ServiceId::new("unexpected")),
            2 => changed.credential.service_scope.as_mut().unwrap().revision += 1,
            _ => changed.lifecycle.next_authority += 1,
        }
        put(&store, &changed).await;
        assert_eq!(
            store
                .advance_access_request(
                    applied.lease.as_ref().unwrap(),
                    AgentAccessPhase::Updating,
                    AgentAccessPhase::Notifying,
                    progress.clone()
                )
                .await,
            Err(AgentAccessError::Conflict),
            "case {case}"
        );
    }
    put(&store, &ready).await;
    let result = store
        .advance_access_request(
            applied.lease.as_ref().unwrap(),
            AgentAccessPhase::Updating,
            AgentAccessPhase::Notifying,
            progress,
        )
        .await
        .unwrap();
    assert_eq!(result.phase, AgentAccessPhase::Notifying);
    assert_eq!(result.progress.hot_update, applied.progress.hot_update);
    client.database(&name).drop().await.unwrap();
}
