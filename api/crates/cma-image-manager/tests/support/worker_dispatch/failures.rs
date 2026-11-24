use super::*;
use mongodb::bson::Document;
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::{atomic::AtomicBool, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn repeated_defer_conflicts_retain_rejection_after_the_tick_budget() {
    let (updates, mut received) = mpsc::unbounded_channel();
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let pause = Arc::new(AtomicBool::new(true));
    let paused = pause.clone();
    let handler = EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        let rejecting = event
            .command
            .get_array("updates")
            .ok()
            .and_then(|updates| updates.first())
            .and_then(|update| update.as_document())
            .and_then(|update| update.get_document("u").ok())
            .and_then(|update| update.get_document("$set").ok())
            .is_some_and(|set| set.get_str("error").ok() == Some("submission_rejected"));
        if rejecting && paused.load(Ordering::SeqCst) {
            updates.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }
    });
    let fixture = Fixture::with_handler(Some(handler)).await;
    let build = fixture.build().await;
    let pending = fixture.submitting(true).await;
    let submission = pending.submission.clone();
    submission.release.notify_one();
    for _ in 0..3 {
        tokio::time::timeout(std::time::Duration::from_secs(10), received.recv())
            .await
            .unwrap()
            .unwrap();
        let current = fixture.observer.build("alice", &build.id).await.unwrap();
        fixture.observer.defer_observation(&current).await.unwrap();
        release.send(()).unwrap();
    }
    let (mut worker, result) = pending.finish().await;
    assert!(matches!(result, Err(Error::Conflict)), "{result:?}");
    assert_eq!(
        fixture
            .observer
            .build("alice", &build.id)
            .await
            .unwrap()
            .phase,
        BuildPhase::Dispatching
    );
    pause.store(false, Ordering::SeqCst);
    let executor: Executor = submission.clone();
    worker.tick(&fixture.dispatcher, &executor).await.unwrap();
    assert_eq!(
        fixture
            .observer
            .build("alice", &build.id)
            .await
            .unwrap()
            .phase,
        BuildPhase::Failed
    );
    assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn known_write_failure_retains_rejection_for_the_next_tick() {
    write_failure(false).await;
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn unknown_write_acknowledgement_reconciles_the_applied_rejection() {
    write_failure(true).await;
}

async fn write_failure(applied: bool) {
    let fixture = Fixture::new().await;
    let build = fixture.build().await;
    let pending = fixture.submitting(true).await;
    let submission = pending.submission.clone();
    let mut data = doc! {"failCommands":["update"],"appName":&fixture.app};
    if applied {
        data.insert(
            "writeConcernError",
            doc! {"code":64,"codeName":"WriteConcernFailed","errmsg":"synthetic acknowledgement loss"},
        );
    } else {
        data.insert("errorCode", 2);
    }
    fixture
        .admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":1},"data":data
        })
        .await
        .unwrap();
    let (mut worker, result) = pending.finish().await;
    fixture
        .admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"off"
        })
        .await
        .unwrap();
    let Err(Error::Database(error)) = result else {
        panic!("expected injected database error, got {result:?}");
    };
    if applied {
        assert!(
            matches!(error.kind.as_ref(),
            mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteConcernError(error))
                if error.code == 64),
            "{error:?}"
        );
        // The failpoint skips the normal majority wait. Commit a later write
        // before checking that the earlier, unacknowledged effect is visible.
        fixture
            .db
            .run_command(doc! {"insert":"visibility",
            "documents":[{"_id":"after-rejection"}],"writeConcern":{"w":"majority"}})
            .await
            .unwrap();
    } else {
        assert!(
            matches!(error.kind.as_ref(),
            mongodb::error::ErrorKind::Command(error) if error.code == 2),
            "{error:?}"
        );
    }
    let before = fixture.observer.build("alice", &build.id).await.unwrap();
    assert_eq!(
        before.phase,
        if applied {
            BuildPhase::Failed
        } else {
            BuildPhase::Dispatching
        }
    );
    let executor: Executor = submission.clone();
    worker.tick(&fixture.dispatcher, &executor).await.unwrap();
    let settled = fixture.observer.build("alice", &build.id).await.unwrap();
    assert_eq!(settled.phase, BuildPhase::Failed);
    assert_eq!(settled.error.as_deref(), Some("submission_rejected"));
    if applied {
        assert_eq!(settled, before);
    }
    assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn rejection_refuses_changed_request_identity_without_losing_the_result() {
    let fixture = Fixture::new().await;
    let builds = fixture.db.collection::<Document>(schema::BUILDS);
    for change in [
        doc! {"owner":"bob"},
        doc! {"image_id":cma_image_manager::model::id("img")},
        doc! {"request_key_hash":"b".repeat(64)},
        doc! {"request_sha256":"b".repeat(64)},
        doc! {"source.commit":"b".repeat(40)},
        doc! {"created_at_ms":1_i64},
    ] {
        let build = fixture.build().await;
        let pending = fixture.submitting(true).await;
        let original = pending.claim.clone();
        let submission = pending.submission.clone();
        builds
            .update_one(doc! {"_id":&build.id}, doc! {"$set":change.clone()})
            .await
            .unwrap();
        let (mut worker, result) = pending.finish().await;
        assert!(
            matches!(result, Err(Error::Conflict | Error::NotFound)),
            "{result:?}"
        );
        let changed = builds
            .find_one(doc! {"_id":&build.id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(changed.get_str("phase").unwrap(), "dispatching");
        let executor: Executor = submission.clone();
        assert!(worker.tick(&fixture.dispatcher, &executor).await.is_err());
        assert_eq!(
            builds
                .find_one(doc! {"_id":&build.id})
                .await
                .unwrap()
                .unwrap(),
            changed
        );
        builds
            .replace_one(
                doc! {"_id":&build.id},
                mongodb::bson::serialize_to_document(&original).unwrap(),
            )
            .await
            .unwrap();
        worker.tick(&fixture.dispatcher, &executor).await.unwrap();
        assert_eq!(
            fixture
                .observer
                .build("alice", &build.id)
                .await
                .unwrap()
                .phase,
            BuildPhase::Failed
        );
        assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    }
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn rejection_matches_reordered_source_fields() {
    let fixture = Fixture::new().await;
    let builds = fixture.db.collection::<Document>(schema::BUILDS);
    for content in [None, Some("FROM scratch\n".into())] {
        let build = fixture.build_with_content(content).await;
        let pending = fixture.submitting(true).await;
        let original = &pending.claim.source;
        let mut source = doc! {
            "context": &original.context,
            "dockerfile": &original.dockerfile,
            "commit": &original.commit,
            "repository": &original.repository,
        };
        if let Some(content) = &original.dockerfile_content {
            source.insert("dockerfile_content", content);
        }
        builds
            .update_one(doc! {"_id":&build.id}, doc! {"$set":{"source":source}})
            .await
            .unwrap();
        let submission = pending.submission.clone();
        pending.finish().await.1.unwrap();
        let settled = fixture.observer.build("alice", &build.id).await.unwrap();
        assert_eq!(settled.phase, BuildPhase::Failed);
        assert_eq!(settled.error.as_deref(), Some("submission_rejected"));
        assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    }
    fixture.cleanup().await;
}

struct ContinuingExecutor {
    queued: String,
    running: String,
    submissions: AtomicUsize,
}

#[async_trait::async_trait]
impl BuildExecutor for ContinuingExecutor {
    async fn submit(&self, build: &Build) -> Result<()> {
        assert_eq!(build.id, self.queued);
        assert_eq!(self.submissions.fetch_add(1, Ordering::SeqCst), 0);
        Err(Error::BuildRejected)
    }

    async fn observe(&self, build: &Build) -> Result<Option<Observation>> {
        assert_eq!(build.id, self.running);
        Ok(Some(Observation {
            id: "running-provider-build".into(),
            phase: BuildPhase::Succeeded,
            digest: Some(format!("sha256:{}", "a".repeat(64))),
        }))
    }

    async fn logs(&self, _: &Build) -> Result<serde_json::Value> {
        unreachable!("the worker does not fetch logs")
    }
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn unsettled_rejection_leaves_other_builds_free_to_progress() {
    let fixture = Fixture::new().await;
    let builds = fixture.db.collection::<Document>(schema::BUILDS);
    let held = fixture.build().await;
    let pending = fixture.submitting(true).await;
    let original = pending.claim.clone();
    let submission = pending.submission.clone();
    builds
        .update_one(
            doc! {"_id":&held.id},
            doc! {"$set":{"source.dockerfile_content":"FROM scratch\n"}},
        )
        .await
        .unwrap();
    let changed = builds
        .find_one(doc! {"_id": &held.id})
        .await
        .unwrap()
        .unwrap();
    let (mut worker, result) = pending.finish().await;
    assert!(matches!(result, Err(Error::Conflict)), "{result:?}");

    let running = fixture.build().await;
    let claim = fixture.observer.claim_build().await.unwrap().unwrap();
    assert_eq!(claim.id, running.id);
    assert!(fixture
        .observer
        .observe_build(&claim, "running-provider-build", BuildPhase::Running, None)
        .await
        .unwrap());
    let queued = fixture.build().await;
    let continuing = Arc::new(ContinuingExecutor {
        queued: queued.id.clone(),
        running: running.id.clone(),
        submissions: AtomicUsize::new(0),
    });
    let executor: Executor = continuing.clone();
    assert!(matches!(
        worker.tick(&fixture.dispatcher, &executor).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        fixture
            .observer
            .build("alice", &queued.id)
            .await
            .unwrap()
            .phase,
        BuildPhase::Failed
    );
    assert_eq!(
        fixture
            .observer
            .build("alice", &running.id)
            .await
            .unwrap()
            .phase,
        BuildPhase::Succeeded
    );
    assert_eq!(
        builds
            .find_one(doc! {"_id": &held.id})
            .await
            .unwrap()
            .unwrap(),
        changed
    );
    assert_eq!(continuing.submissions.load(Ordering::SeqCst), 1);

    builds
        .replace_one(
            doc! {"_id":&held.id},
            mongodb::bson::serialize_to_document(&original).unwrap(),
        )
        .await
        .unwrap();
    worker.tick(&fixture.dispatcher, &executor).await.unwrap();
    assert_eq!(
        fixture
            .observer
            .build("alice", &held.id)
            .await
            .unwrap()
            .phase,
        BuildPhase::Failed
    );
    assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(continuing.submissions.load(Ordering::SeqCst), 1);
    fixture.cleanup().await;
}
