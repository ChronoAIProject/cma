//! Visitor readiness remains independent of erased publisher analytics.
use super::*;

pub(super) async fn empty_message_fixture() -> Fixture {
    let mut f = continuity_fixture().await;
    let definition = f
        .seed
        .store
        .trigger(&f.public.trigger_id, &f.seed.owner)
        .await
        .unwrap()
        .unwrap();
    let mut draft = definition.draft;
    draft.first_message.clear();
    let saved = f
        .seed
        .store
        .save_draft(
            &f.public.trigger_id,
            &f.seed.owner,
            definition.draft_version,
            draft,
            NOW.into(),
        )
        .await
        .unwrap();
    f.public = f
        .seed
        .store
        .publish_trigger(
            &f.public.trigger_id,
            &f.seed.owner,
            saved.draft_version,
            NOW.into(),
        )
        .await
        .unwrap();
    f
}

pub(super) async fn publish_agent_ready(
    f: &Fixture,
    launch: &Launch,
) -> layer2_core::sandbox::Agent {
    let mut agent = f
        .resources
        .get_agent(launch.resolution.agent.id().unwrap())
        .await
        .unwrap()
        .unwrap();
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(layer2_core::now_rfc3339());
    agent.conversation.conversation_id = Some("publisher-erasure-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.work = AgentWorkState::new(AgentWorkStep::None, layer2_core::now_rfc3339());
    agent.revision += 1;
    let db = f.seed.client.database(&f.seed.database);
    let mut tree = f
        .resources
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    tree.status = WorktreeStatus::Ready;
    tree.revision += 1;
    db.collection::<Document>("worktrees")
        .replace_one(
            doc! {"_id":tree.id.as_str()},
            crate::store::worktrees::worktree_document(&tree).unwrap(),
        )
        .await
        .unwrap();
    db.collection::<Document>("agents")
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    agent
}

async fn waiting_for_ready() -> (
    Arc<Fixture>,
    Arc<LaunchWorker>,
    Launch,
    std::time::SystemTime,
) {
    let f = Arc::new(empty_message_fixture().await);
    let launch = f
        .ready_for_agent(f.launch("publisher-ready-counter").await)
        .await;
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("publisher-ready-counter-worker"),
    );
    let now = std::time::SystemTime::now() + std::time::Duration::from_secs(61);
    assert!(worker.run_one(&launch.id, now).await.unwrap());
    let launch = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(launch.work.step, LaunchWorkStep::WaitForAgent);
    publish_agent_ready(&f, &launch).await;
    (f, worker, launch, now)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_ready_commit_survives_each_definition_revision_intermediate_state() {
    for revision_first in [false, true] {
        let (f, worker, launch, now) = waiting_for_ready().await;
        let db = f.seed.client.database(&f.seed.database);
        let definitions = db.collection::<Document>("trigger_definitions");
        if revision_first {
            let definition = definitions
                .find_one(doc! {"_id":launch.trigger_id.as_str()})
                .await
                .unwrap()
                .unwrap();
            erase_publisher(&f).await;
            definitions.insert_one(definition).await.unwrap();
        } else {
            // A missing definition is an erasure intermediate state only when
            // its exact retained revision carries valid erasure authority.
            db.collection::<Document>("trigger_definition_revisions")
                .update_many(
                    doc! {"trigger_id":launch.trigger_id.as_str()},
                    doc! {"$set":{
                        "account_erasure_fenced":true,
                        "account_erasure_fenced_at":mongodb::bson::DateTime::now(),
                    }},
                )
                .await
                .unwrap();
            definitions
                .delete_one(doc! {"_id":launch.trigger_id.as_str()})
                .await
                .unwrap();
        }
        assert!(worker
            .run_one(&launch.id, now + std::time::Duration::from_secs(1))
            .await
            .unwrap());
        let ready = f
            .seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.state, LaunchState::Ready);
        assert!(ready.ready_at.is_some());
        assert!(ready.failure.is_none());
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_counter_rejects_a_missing_definition_without_positive_erasure_evidence() {
    let (f, worker, launch, now) = waiting_for_ready().await;
    let db = f.seed.client.database(&f.seed.database);
    db.collection::<Document>("trigger_definitions")
        .delete_one(doc! {"_id":launch.trigger_id.as_str()})
        .await
        .unwrap();
    let result = worker
        .run_one(&launch.id, now + std::time::Duration::from_secs(1))
        .await;
    assert!(matches!(
        result,
        Err(crate::trigger::LaunchWorkerError::Store(
            LaunchStoreError::Corrupt(_)
        ))
    ));
    let unchanged = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.state, launch.state);
    assert_eq!(unchanged.work.step, LaunchWorkStep::WaitForAgent);
    assert!(unchanged.ready_at.is_none());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_counter_accepts_a_missing_definition_with_a_valid_publisher_tombstone() {
    let (f, worker, launch, now) = waiting_for_ready().await;
    let db = f.seed.client.database(&f.seed.database);
    erase_publisher(&f).await;
    assert!(worker
        .run_one(&launch.id, now + std::time::Duration::from_secs(1))
        .await
        .unwrap());
    let ready = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.state, LaunchState::Ready);
    assert!(ready.ready_at.is_some());
    assert!(ready.failure.is_none());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_ready_counter_keeps_ordinary_revision_corruption_strict() {
    let (f, worker, launch, now) = waiting_for_ready().await;
    let db = f.seed.client.database(&f.seed.database);
    let revisions = db.collection::<Document>("trigger_definition_revisions");
    let filter =
        doc! {"trigger_id":launch.trigger_id.as_str(),"record.digest":&launch.revision_digest};
    let row = revisions.find_one(filter.clone()).await.unwrap().unwrap();
    revisions
        .update_one(
            filter.clone(),
            doc! {"$set":{
                "published_at":"2026-11-15T00:00:00Z","record.published_at":"2026-11-15T00:00:00Z",
            }},
        )
        .await
        .unwrap();
    assert!(matches!(
        worker
            .run_one(&launch.id, now + std::time::Duration::from_secs(1))
            .await,
        Err(crate::trigger::LaunchWorkerError::Store(
            LaunchStoreError::Corrupt(_)
        ))
    ));
    let unchanged = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert!(unchanged.ready_at.is_none());
    assert_eq!(unchanged.work.step, LaunchWorkStep::WaitForAgent);
    revisions.replace_one(filter, row).await.unwrap();
    assert_eq!(
        f.seed
            .store
            .trigger(&launch.trigger_id, &f.seed.owner)
            .await
            .unwrap()
            .unwrap()
            .stats
            .agents_ready_total,
        0
    );
    assert!(worker
        .run_one(&launch.id, now + std::time::Duration::from_secs(902))
        .await
        .unwrap());
    assert_eq!(
        f.seed
            .store
            .trigger(&launch.trigger_id, &f.seed.owner)
            .await
            .unwrap()
            .unwrap()
            .stats
            .agents_ready_total,
        1
    );
    assert_eq!(
        f.seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        LaunchState::Ready
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_ready_continuation_preserves_definition_fence() {
    let (f, worker, launch, now) = waiting_for_ready().await;
    let db = f.seed.client.database(&f.seed.database);
    let definitions = db.collection::<Document>("trigger_definitions");
    let filter = doc! {"_id":launch.trigger_id.as_str()};
    let fenced_at = mongodb::bson::DateTime::now();
    definitions
        .update_one(
            filter.clone(),
            doc! {"$set":{
                "account_erasure_fenced":true,
                "account_erasure_fenced_at":fenced_at,
            }},
        )
        .await
        .unwrap();
    // The existing direct counter mutations already preserve unrelated fields.
    f.seed
        .store
        .record_launch(&launch.trigger_id)
        .await
        .unwrap();
    f.seed
        .store
        .record_agent_ready(&launch.trigger_id)
        .await
        .unwrap();
    let before = definitions.find_one(filter.clone()).await.unwrap().unwrap();
    assert_eq!(before.get_bool("account_erasure_fenced").ok(), Some(true));
    assert_eq!(
        before.get_datetime("account_erasure_fenced_at").unwrap(),
        &fenced_at
    );
    assert!(worker
        .run_one(&launch.id, now + std::time::Duration::from_secs(1))
        .await
        .unwrap());
    let ready = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.state, LaunchState::Ready);
    let after = definitions.find_one(filter).await.unwrap().unwrap();
    assert_eq!(after.get_bool("account_erasure_fenced").ok(), Some(true));
    assert_eq!(
        after.get_datetime("account_erasure_fenced_at").unwrap(),
        &fenced_at
    );
    let definition = f
        .seed
        .store
        .trigger(&launch.trigger_id, &f.seed.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(definition.stats.launches_total, 2);
    assert_eq!(definition.stats.agents_ready_total, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_new_launch_refuses_a_fenced_definition() {
    let f = empty_message_fixture().await;
    let db = f.seed.client.database(&f.seed.database);
    db.collection::<Document>("trigger_definitions")
        .update_one(
            doc! {"_id":f.public.trigger_id.as_str()},
            doc! {"$set":{
                "account_erasure_fenced":true,
                "account_erasure_fenced_at":mongodb::bson::DateTime::now(),
            }},
        )
        .await
        .unwrap();
    let before = db
        .collection::<Document>("trigger_launches")
        .count_documents(doc! {})
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .create_or_resume_as(
                &f.identity,
                &f.public.public_id,
                f.request("fenced-new-launch"),
            )
            .await,
        Err(LaunchServiceError::TriggerRetired)
    ));
    assert_eq!(
        db.collection::<Document>("trigger_launches")
            .count_documents(doc! {})
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        f.seed
            .store
            .trigger(&f.public.trigger_id, &f.seed.owner)
            .await
            .unwrap()
            .unwrap()
            .stats
            .launches_total,
        0
    );
    db.drop().await.unwrap();
}
