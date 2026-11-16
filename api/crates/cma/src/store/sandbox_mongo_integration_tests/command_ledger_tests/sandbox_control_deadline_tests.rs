use super::*;
use layer2_core::sandbox::{SandboxLifecycleResult, SandboxLifecycleResultState};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_pause_and_retirement_deadlines_survive_application_and_store_restart() {
    use layer2_core::sandbox::store::SandboxEventStore;
    use layer2_core::sandbox_usage_service::SandboxUsageStore;
    for command in [
        SandboxControl::Pause,
        SandboxControl::Revive,
        SandboxControl::Retire,
    ] {
        let (client, name) = database("sandbox_controls_deadline").await;
        let mut f = Fixture::new(&client, &name).await;
        if command == SandboxControl::Revive {
            let mut paused = f.parent.clone();
            paused.status = SandboxStatus::Paused;
            paused.runtime = None;
            paused.work.step = layer2_core::sandbox::SandboxWorkStep::None;
            paused.lifecycle.checkpoints.latest = Some(crate::store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::checkpoint_for_agents(&[], 1));
            paused.revision += 1;
            f.store
                .save_sandbox(f.parent.revision, paused.clone())
                .await
                .unwrap();
            f.parent = paused;
        }
        let owner = context("owner");
        let mut record = f
            .service
            .admit(
                &owner,
                "fixture",
                SandboxControlSubmission {
                    sandbox_id: f.parent.id.clone(),
                    command,
                    expected_revision: f.parent.revision,
                    idempotency_key: "deadline".into(),
                },
            )
            .await
            .unwrap();
        let now = SystemTime::now();
        record.admitted_at = layer2_core::rfc3339_at(now - Duration::from_secs(100));
        record.queue_deadline = layer2_core::rfc3339_at(now + Duration::from_secs(200));
        record.validate().unwrap();
        // Simulate a retained queued receipt without waiting on a wall-clock timer.
        f.store.sandbox_controls.update_one(doc! {"_id":record.envelope.command_id.as_str()}, doc! {"$set": {
            "record.admitted_at": &record.admitted_at, "record.queue_deadline": &record.queue_deadline, "queue_deadline": &record.queue_deadline,
        }}).await.unwrap();
        // Prior operation completion is logged while the control is queued.
        // Admission precedes this event; application must not backdate the next.
        f.store
            .append_sandbox_event(
                &f.parent.id,
                layer2_core::rfc3339_at(now - Duration::from_secs(50)),
                layer2_core::sandbox::SandboxEventKind::WorkStepCompleted {
                    step: layer2_core::sandbox::SandboxWorkStep::RunLifecycleOperation,
                },
            )
            .await
            .unwrap();
        let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let retained = replica
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained, record);
        let before = SandboxLifecycleResult::queued_control(&retained, now).unwrap();
        let proof = f
            .access
            .authorize_sandbox_control(&owner, "fixture", &retained)
            .await
            .unwrap();
        let applied = f
            .store
            .apply_sandbox_control(&retained, proof)
            .await
            .unwrap();
        let root = replica.get_sandbox(&f.parent.id).await.unwrap().unwrap();
        let SandboxControlStatus::Applied { at: applied_at, .. } = applied.status else {
            panic!("queued {command:?} was not applied: {:?}", applied.status);
        };
        assert!(applied_at > record.admitted_at);
        assert_eq!(root.updated_at, applied_at);
        assert_eq!(root.work.ready_at, applied_at);
        let events = replica.sandbox_events(&root.id, 0, 100).await.unwrap();
        assert!(events
            .windows(2)
            .all(|pair| pair[0].appended_at <= pair[1].appended_at));
        let event = events.last().unwrap();
        assert_eq!(event.appended_at, applied_at);
        assert!(matches!(
            event.kind,
            layer2_core::sandbox::SandboxEventKind::StatusChanged { .. }
        ));
        let usage = replica.usage(&root.id).await.unwrap().unwrap();
        let applied_seconds = layer2_core::seconds_from_rfc3339(&applied_at).unwrap();
        assert_eq!(usage.last_observed_at, applied_seconds);
        let intervals = replica
            .usage_events(&root.id, &PageRequest::new(100, None).unwrap())
            .await
            .unwrap();
        assert_eq!(intervals.items.last().unwrap().ended_at, applied_seconds);
        assert!(intervals
            .items
            .iter()
            .all(|interval| interval.ended_at >= interval.started_at));
        let admitted = replica.retirement_admitted_at(&root.id).await.unwrap();
        let result =
            SandboxLifecycleResult::observe(&root, admitted.as_deref(), now, true).unwrap();
        assert_eq!(result.started_at, before.started_at);
        assert_eq!(result.result_due_at, before.result_due_at);
        assert_eq!(
            result.state,
            SandboxLifecycleResultState::RecoverablePending
        );
        client.database(&name).drop().await.unwrap();
    }
}
