use super::*;
use crate::store::adoption_probe::{self, Boundary};
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, WorktreeStore};
use std::time::Duration;

const WITNESS: &str = "pending_worker_log_adoption_v1";

async fn selected(app: Option<String>) -> (Fixture, Agent) {
    let fixture = Fixture::with_adoption(app, true).await;
    let full = initialize_connection(&fixture).await;
    adoption_policy(&fixture.client, &fixture.name, full.id.as_str(), 0, true).await;
    (fixture, full)
}

async fn wait(barrier: &adoption_probe::Barrier) {
    tokio::time::timeout(Duration::from_secs(40), barrier.reached.notified())
        .await
        .expect("worker did not reach the admission barrier");
}

async fn wait_for_commit(barrier: &adoption_probe::Barrier) {
    // Reaching the commit barrier can require several passes over the full
    // legacy log on the CI replica set.
    tokio::time::timeout(Duration::from_secs(120), barrier.reached.notified())
        .await
        .expect("worker did not reach the commit barrier");
}

async fn no_publication(fixture: &Fixture) {
    let db = fixture.client.database(&fixture.name);
    for collection in [
        "agent_log_publications",
        "agent_log_segments",
        "agent_log_heads",
    ] {
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(fixture.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn closure_before_and_after_admission() {
    for boundary in [Boundary::BeforeAdmission, Boundary::Admitted] {
        let (fixture, full) = selected(None).await;
        let database = fixture.name.clone();
        let client = fixture.client.clone();
        let barrier = adoption_probe::arm(&fixture.name, boundary);
        let finish = tokio::spawn(async move {
            let result = finish_now(&fixture).await;
            (fixture, result)
        });
        wait(&barrier).await;
        adoption_policy(&client, &database, full.id.as_str(), 1, false).await;
        barrier.resume.notify_one();
        let (fixture, result) = finish.await.unwrap();
        result.unwrap();
        if boundary == Boundary::BeforeAdmission {
            assert_capacity_failure(&fixture, &full).await;
            no_publication(&fixture).await;
        } else {
            check_publication(&fixture, &full, 1).await;
        }
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn wrong_explicit_cohort_keeps_the_ordinary_capacity_result() {
    let fixture = Fixture::with_adoption(None, true).await;
    let full = initialize_connection(&fixture).await;
    crate::store::native_prepared::tests::open_ready_fixture::adoption_policies(
        &fixture.client,
        &fixture.name,
        full.id.as_str(),
        "different-synthetic-agent",
        0,
        true,
    )
    .await;
    finish_now(&fixture).await.unwrap();
    assert_capacity_failure(&fixture, &full).await;
    no_publication(&fixture).await;
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn original_claim_and_stale_revision_fence_are_preserved_during_staging() {
    let (fixture, full) = selected(None).await;
    let fixture_client = fixture.client.clone();
    let fixture_name = fixture.name.clone();
    let retained = fixture.store.clone();
    let barrier = adoption_probe::arm(&fixture.name, Boundary::Admitted);
    let finish = tokio::spawn(async move {
        let result = finish_now(&fixture).await;
        (fixture, result)
    });
    wait(&barrier).await;
    let now = SystemTime::now();
    assert!(retained
        .claim_agent_work(
            &full.id,
            "other-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(120))
        )
        .await
        .unwrap()
        .is_none());
    let row = fixture_client
        .database(&fixture_name)
        .collection::<Document>("agents")
        .find_one(doc! {"_id": full.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_document(WITNESS).unwrap().get_str("state").unwrap(),
        "admitted"
    );
    let mut replacement = retained.get_agent(&full.id).await.unwrap().unwrap();
    replacement.revision += 1;
    let lease = replacement.work.lease.as_mut().unwrap();
    lease.expires_at = rfc3339_at(now + Duration::from_secs(119));
    lease.holder = "different-worker".into();
    lease.fence += 1;
    replacement.work.next_fence += 1;
    retained
        .save_agent(replacement.revision - 1, replacement.clone())
        .await
        .unwrap();
    barrier.resume.notify_one();
    let (fixture, result) = finish.await.unwrap();
    assert!(result.is_err());
    no_publication(&fixture).await;
    assert_eq!(current(&fixture).await, replacement);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at disposable replica set"]
async fn lost_publication_acknowledgment_replays_after_closure_restart_and_later_write() {
    let app = format!("pending-worker-ack-{}", uuid::Uuid::new_v4().simple());
    let (fixture, full) = selected(Some(app.clone())).await;
    let admin =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    let db = fixture.name.clone();
    let worker_store = fixture.store.clone();
    let now = SystemTime::now();
    let claimed = worker_store
        .claim_agent_work(
            &full.id,
            "open-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(300)),
        )
        .await
        .unwrap()
        .unwrap();
    let original = fixture
        .worker
        .progress_mutation(
            claimed,
            AgentWorkStep::DispatchFirstMessage,
            now,
            None,
            Vec::new(),
            None,
            None,
        )
        .unwrap();
    let mut parents = fixture.context.publication_parents();
    parents.worktree_revision = worker_store
        .get_worktree(&full.worktree_id)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let barrier = adoption_probe::arm(&db, Boundary::BeforeCommit);
    let finish = tokio::spawn({
        let operation = original.clone();
        async move {
            worker_store
                .commit_agent_connection_work(operation, parents)
                .await
        }
    });
    wait_for_commit(&barrier).await;
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 6_i32},
        "data": {"failCommands": ["commitTransaction"], "appName": app,
            "writeConcernError": {"code": 50, "codeName": "MaxTimeMSExpired", "errmsg": "synthetic lost acknowledgement"}},
    }).await.unwrap();
    barrier.resume.notify_one();
    let result = finish.await.unwrap();
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(result.is_ok() || matches!(result, Err(SandboxStoreError::Unavailable(_))));
    crate::store::test_visibility::majority_barrier(&admin, &db).await;
    let published = check_publication(&fixture, &full, 1).await;
    adoption_policy(&admin, &db, full.id.as_str(), 1, false).await;
    let restarted = MongoSandboxStore::open(&admin, &db).await.unwrap();
    let later_event = AgentLogKind::ConversationGap { frames: 1 };
    restarted
        .append_agent_log_keyed(
            &full.id,
            &AgentLogPublicationIdentity {
                key: "compatible-writer-after-adoption".into(),
                appended_at: rfc3339_at(SystemTime::now()),
                fingerprint: [1; 32],
            },
            later_event,
        )
        .await
        .unwrap();
    let later = restarted.get_agent(&full.id).await.unwrap().unwrap();
    assert_eq!(
        later.conversation.last_seq,
        published.conversation.last_seq + 1
    );
    restarted
        .commit_agent_connection_work(original, parents)
        .await
        .unwrap();
    assert_eq!(restarted.get_agent(&full.id).await.unwrap().unwrap(), later);
    let publications = admin
        .database(&db)
        .collection::<Document>("agent_log_publications");
    assert_eq!(
        publications
            .count_documents(doc! {"expected_seq": 10_000_i64})
            .await
            .unwrap(),
        1
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn owner_cleanup_supersedes_staged_adoption() {
    use layer2_core::sandbox::store::{
        AgentControlResult, AgentControlStore, AgentControlTarget, AgentDeleteRequest,
        AgentStopRequest,
    };
    use layer2_core::sandbox::AgentStopReason;
    for delete in [false, true] {
        let (fixture, full) = selected(None).await;
        let store = if delete {
            Arc::new(
                MongoSandboxStore::open(&fixture.client, &fixture.name)
                    .await
                    .unwrap(),
            )
        } else {
            fixture.store.clone()
        };
        let worker = fixture.worker.clone();
        let barrier = adoption_probe::arm(&fixture.name, Boundary::Admitted);
        let finish = tokio::spawn(async move {
            let result = finish_now(&fixture).await;
            (fixture, result)
        });
        wait(&barrier).await;
        let claimed = store.get_agent(&full.id).await.unwrap().unwrap();
        let target = AgentControlTarget::from(&claimed);
        let requested_at = rfc3339_at(SystemTime::now());
        let AgentControlResult::Scheduled(cleanup) = store
            .schedule_agent_stop(AgentStopRequest {
                target,
                requested_at,
                reason: AgentStopReason::Owner,
            })
            .await
            .unwrap()
        else {
            panic!("expected fresh stop");
        };
        let stopped = tokio::time::timeout(
            Duration::from_secs(30),
            publish_confirmed_stop(&worker, &store, &cleanup),
        )
        .await
        .expect("stop publication must finish while adoption is paused");
        assert_eq!(stopped.status, AgentStatus::Stopped);
        assert_eq!(stopped.work.step, AgentWorkStep::None);
        let cleanup = if delete {
            let AgentControlResult::Scheduled(deletion) = store
                .schedule_agent_delete(AgentDeleteRequest {
                    target: AgentControlTarget::from(&stopped),
                    requested_at: rfc3339_at(SystemTime::now()),
                })
                .await
                .unwrap()
            else {
                panic!("expected fresh delete");
            };
            deletion
        } else {
            stopped
        };
        barrier.resume.notify_one();
        let (fixture, result) = finish.await.unwrap();
        assert!(result.is_err());
        no_publication(&fixture).await;
        assert_eq!(current(&fixture).await, cleanup);
        assert_eq!(
            cleanup.conversation.conversation_id,
            full.conversation.conversation_id
        );
        fixture.cleanup().await;
    }
}

async fn publish_confirmed_stop(
    worker: &AgentWorker,
    store: &MongoSandboxStore,
    cleanup: &Agent,
) -> Agent {
    use layer2_core::sandbox::{AgentStopReason, SandboxEventKind};
    for (step, next) in [
        (AgentWorkStep::FenceInteraction, AgentWorkStep::CloseProcess),
        (AgentWorkStep::CloseProcess, AgentWorkStep::PublishStopped),
        (AgentWorkStep::PublishStopped, AgentWorkStep::None),
    ] {
        let now = SystemTime::now();
        let stamp = rfc3339_at(now);
        let claim = store
            .claim_agent_work(
                &cleanup.id,
                "open-worker",
                stamp.clone(),
                rfc3339_at(now + Duration::from_secs(120)),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.work.step, step);
        let evidence = (step == AgentWorkStep::CloseProcess).then(|| RequestEvidence {
            step,
            operation_id: format!("agent:{}:close:v1", claim.id),
            request_sha256: "b".repeat(64),
        });
        let mut mutation = worker
            .progress_mutation(claim.clone(), next, now, evidence, Vec::new(), None, None)
            .unwrap();
        if step == AgentWorkStep::PublishStopped {
            mutation.agent.revision -= 1;
            mutation
                .agent
                .transition(AgentStatus::Stopped, stamp, Some(AgentStopReason::Owner))
                .unwrap();
            mutation.agent_logs = vec![AgentLogKind::Stopped {
                reason: "owner".into(),
            }];
            mutation.sandbox_event = Some(SandboxEventKind::AgentStatusChanged {
                agent_id: cleanup.id.clone(),
                from: claim.status,
                to: AgentStatus::Stopped,
            });
        }
        store.commit_agent_work(mutation).await.unwrap();
    }
    store.get_agent(&cleanup.id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at disposable replica set"]
async fn unresolved_publication_blocks_replacement_before_and_after_original_lease_expiry() {
    let app = format!(
        "pending-worker-unresolved-{}",
        uuid::Uuid::new_v4().simple()
    );
    let (fixture, full) = selected(Some(app.clone())).await;
    let admin =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    let db = fixture.name.clone();
    let barrier = adoption_probe::arm(&db, Boundary::BeforeCommit);
    let mut finish = tokio::spawn(async move {
        let result = finish_now(&fixture).await;
        (fixture, result)
    });
    tokio::select! {
        () = wait_for_commit(&barrier) => {}
        result = &mut finish => {
            panic!("worker finished before the commit failpoint was installed: {:?}", result.unwrap().1);
        }
    }
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "alwaysOn",
            "data": {"failCommands": ["commitTransaction"], "appName": app,
                "errorCode": 50, "errorLabels": ["UnknownTransactionCommitResult"]},
        })
        .await
        .unwrap();
    barrier.resume.notify_one();
    let (fixture, result) = finish.await.unwrap();
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    no_publication(&fixture).await;
    let restarted = MongoSandboxStore::open(&admin, &db).await.unwrap();
    let original = restarted.get_agent(&full.id).await.unwrap().unwrap();
    let lease = original.work.lease.as_ref().unwrap();
    let expires = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&lease.expires_at).unwrap());
    for at in [SystemTime::now(), expires + Duration::from_secs(1)] {
        assert!(restarted
            .claim_agent_work(
                &full.id,
                "replacement",
                rfc3339_at(at),
                rfc3339_at(at + Duration::from_secs(300))
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            restarted.get_agent(&full.id).await.unwrap().as_ref(),
            Some(&original)
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn uncertain_retained_output_never_acknowledges_or_publishes() {
    use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
    let (fixture, full) = selected(None).await;
    let now = SystemTime::now();
    let claimed = fixture
        .store
        .claim_agent_work(
            &full.id,
            "open-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(120)),
        )
        .await
        .unwrap()
        .unwrap();
    let (updates, mut input) = tokio::sync::mpsc::channel(1);
    let (ack, mut received_ack) = tokio::sync::mpsc::channel(1);
    updates
        .send(ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                    cursor: 1,
                    native_conversation_id: "different-synthetic-conversation".into(),
                    payload_sha256: "b".repeat(64),
                },
                ack,
            ),
            update: None,
        })
        .await
        .unwrap();
    let result = fixture
        .worker
        .drain_connection_output(
            &claimed,
            std::future::pending::<Result<(), _>>(),
            &mut input,
        )
        .await;
    assert!(result.is_err());
    assert!(received_ack.try_recv().is_err());
    assert_eq!(current(&fixture).await, claimed);
    no_publication(&fixture).await;
    fixture.cleanup().await;
}
