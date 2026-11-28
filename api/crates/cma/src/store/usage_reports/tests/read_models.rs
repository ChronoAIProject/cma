use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_sandbox_quarantine_preserves_usage_and_healthy_metadata() {
    let store = fixture().await;
    let at = now();
    let first = state(1, at - 600);
    let second = state(2, at - 600);
    seed(&store, &[first.clone(), second.clone()]).await;
    let mut healthy =
        crate::store::sandbox_mongo_integration_tests::sandbox(second.sandbox_id.as_str());
    healthy.owner = second.owner.clone();
    store.collection("sandboxes").insert_many([
        doc! { "_id": first.sandbox_id.as_str(), (crate::store::sandbox_quarantine::FIELD): {} },
        crate::store::sandboxes::sandbox_document(&healthy, 0).unwrap(),
    ]).await.unwrap();

    let mut session = store.snapshot().await.unwrap();
    let query = ReportInput::default().validate(at).unwrap();
    let rows = store
        .inventory(&mut session, &scope("operator", true), &query, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].state.sandbox_id, first.sandbox_id);
    assert!(rows[0].sandbox.is_none());
    assert_eq!(rows[1].sandbox.as_ref(), Some(&healthy));
    drop(session);
    store.db.drop().await.unwrap();
}

pub(super) mod measured_usage;

pub(super) fn binding(id: &str, kind: BindingKind, at: u64) -> Binding {
    Binding {
        id: String::new(),
        schema_version: 1,
        revision: 1,
        deployment: "test".into(),
        cluster: "cluster".into(),
        namespace: "namespace".into(),
        native_sandbox_uid: "native-uid".into(),
        claim_uid: "claim-uid".into(),
        sandbox_id: id.into(),
        owner_subject: "owner-1".into(),
        pod_name: Some("cold-pod".into()),
        pod_uid: Some("pod-uid".into()),
        runtime_container_id: Some("containerd://first".into()),
        restart_count: Some(0),
        runtime_generation: Some(1),
        pvc_name: "home".into(),
        pvc_uid: "pvc-uid".into(),
        kind,
        cpu_limit_cores: Some(2.),
        memory_limit_bytes: Some(1024.),
        valid_from: at,
        valid_until: None,
        observed_at: at,
        proof_version: 1,
        proof_status: ProofStatus::Verified,
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_bindings_preserve_independent_home_and_fence_conflicts_and_expiry() {
    let store = fixture().await;
    assert!(store
        .lease("observer", "fixture-observer", 70)
        .await
        .unwrap());
    let at = now() - 1000;
    let id = "sbx_00000000000000000000000000000001";
    let compute = binding(id, BindingKind::Compute, at);
    let home = binding(id, BindingKind::Home, at);
    store
        .reconcile_bindings(
            "fixture-observer",
            id,
            vec![compute.clone(), home.clone()],
            at,
        )
        .await
        .unwrap();
    let revision = store.mapping_revision().await.unwrap();
    store
        .reconcile_bindings(
            "fixture-observer",
            id,
            vec![compute.clone(), home.clone()],
            at + 60,
        )
        .await
        .unwrap();
    assert_eq!(store.mapping_revision().await.unwrap(), revision);
    let mut conflict = home.clone();
    conflict.sandbox_id = "another".into();
    conflict.owner_subject = "other-owner".into();
    assert!(store
        .reconcile_bindings("fixture-observer", "another", vec![conflict], at + 60)
        .await
        .is_err());
    store
        .reconcile_bindings("fixture-observer", id, vec![home.clone()], at + 120)
        .await
        .unwrap();
    let records = store
        .bindings(
            &[id.into()],
            Range {
                from: at,
                to: at + 500,
            },
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records
            .iter()
            .find(|r| r.kind == BindingKind::Compute)
            .unwrap()
            .valid_until,
        Some(at + 60)
    );
    assert_eq!(
        records
            .iter()
            .find(|r| r.kind == BindingKind::Home)
            .unwrap()
            .valid_until,
        None
    );
    let revision = store.mapping_revision().await.unwrap();
    store
        .expire_missing_bindings("fixture-observer", at + 211)
        .await
        .unwrap();
    assert_eq!(store.mapping_revision().await.unwrap(), revision + 1);
    store
        .expire_missing_bindings("fixture-observer", at + 212)
        .await
        .unwrap();
    assert_eq!(store.mapping_revision().await.unwrap(), revision + 1);
    let collection = store.collection(super::super::super::sandbox_metric_bindings::COLLECTION);
    let mut cursor = collection.find(doc! {}).await.unwrap();
    while let Some(d) = cursor.try_next().await.unwrap() {
        let b: Binding = decode(&d).unwrap();
        assert_eq!(
            d.get_datetime("expires_at").unwrap().timestamp_millis(),
            ((b.valid_until.unwrap() + IDENTITY_RETENTION_SECONDS) * 1000) as i64
        );
        assert!(!d.contains_key("active_physical"));
    }
    store
        .reconcile_bindings("fixture-observer", id, vec![home], at + 300)
        .await
        .unwrap();
    let active = collection
        .find_one(doc! {"valid_until":null})
        .await
        .unwrap()
        .unwrap();
    assert!(!active.contains_key("expires_at"));
    assert_eq!(decode::<Binding>(&active).unwrap().valid_from, at + 300);
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_rollup_replay_is_idempotent_keeps_success_and_has_no_raw_samples() {
    let store = fixture().await;
    assert!(store.lease("rollups", "fixture-rollups", 70).await.unwrap());
    let hour = now() / 3600 * 3600 - 3600;
    let mut rollup = MetricRollup {
        schema_version: 1,
        algorithm_version: ALGORITHM_VERSION,
        source: SOURCE.into(),
        sandbox_id: "test".into(),
        owner_subject: "owner".into(),
        day: hour / 86_400 * 86_400,
        chunk: ((hour % 86_400) / 3600) as u16,
        mapping_revision: 1,
        cutoff: hour + 3600,
        source_complete: true,
        minutes: vec![Minute {
            from: hour,
            to: hour + 60,
            metrics: [
                Aggregate {
                    integral: 30.,
                    covered_seconds: 60.,
                    expected_seconds: 60.,
                    ..Default::default()
                },
                Aggregate::default(),
                Aggregate::default(),
            ],
        }],
    };
    store
        .save_metric_rollups("rollups", "fixture-rollups", &[rollup.clone()])
        .await
        .unwrap();
    store
        .save_metric_rollups("rollups", "fixture-rollups", &[rollup.clone()])
        .await
        .unwrap();
    rollup.source_complete = false;
    rollup.minutes.clear();
    store
        .save_metric_rollups("rollups", "fixture-rollups", &[rollup])
        .await
        .unwrap();
    let collection = store.collection(super::super::super::sandbox_metric_rollups::COLLECTION);
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1);
    let d = collection.find_one(doc! {}).await.unwrap().unwrap();
    assert!(!d.to_string().contains("samples"));
    assert_eq!(
        d.get_datetime("expires_at").unwrap().timestamp_millis(),
        ((hour + 3600 + RETENTION_SECONDS) * 1000) as i64
    );
    let retained: MetricRollup = decode(&d).unwrap();
    assert_eq!(retained.minutes[0].metrics[0].integral, 30.);
    assert!(retained.source_complete);
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_inventory_uses_one_snapshot_and_progress_is_lease_fenced() {
    let store = fixture().await;
    let mut first = state(1, now() - 600);
    let second = state(2, now() - 600);
    seed(&store, &[first.clone(), second]).await;
    let mut session = store.snapshot().await.unwrap();
    let query = ReportInput::default().validate(now()).unwrap();
    let original = store
        .inventory(&mut session, &scope("operator", true), &query, None)
        .await
        .unwrap();
    first.name = "Changed after snapshot".into();
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .replace_one(
            doc! {"_id":first.sandbox_id.as_str()},
            super::super::super::sandbox_usage::state_document(&first).unwrap(),
        )
        .await
        .unwrap();
    let retained = store
        .inventory(&mut session, &scope("operator", true), &query, None)
        .await
        .unwrap();
    assert_eq!(retained[0].state.name, original[0].state.name);
    assert!(store.progress("replay").await.unwrap().after.is_none());
    let progress = Progress {
        after: Some("last".into()),
        from: 100,
        to: 200,
    };
    assert!(store
        .save_progress("replay", "not-holder", &progress)
        .await
        .is_err());
    assert!(store.lease("replay", "holder", 70).await.unwrap());
    store
        .save_progress("replay", "holder", &progress)
        .await
        .unwrap();
    assert_eq!(
        store.progress("replay").await.unwrap().after,
        Some("last".into())
    );
    store.release("replay", "holder").await.unwrap();
    assert!(store
        .save_progress("replay", "holder", &progress)
        .await
        .is_err());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_diagnostics_exclude_assigned_resources_and_reset_changed_incarnations() {
    let store = fixture().await;
    assert!(store
        .lease("observer", "fixture-observer", 70)
        .await
        .unwrap());
    assert!(store
        .lease("diagnostics", "fixture-diagnostics", 70)
        .await
        .unwrap());
    let at = now() - 600;
    let home = binding("owner-sandbox", BindingKind::Home, at);
    store
        .reconcile_bindings("fixture-observer", "owner-sandbox", vec![home.clone()], at)
        .await
        .unwrap();
    let mut diagnostic = home;
    diagnostic.sandbox_id.clear();
    diagnostic.owner_subject = "unassigned_pool".into();
    assert!(store
        .diagnostic_bindings("fixture-diagnostics", vec![diagnostic.clone()])
        .await
        .unwrap()
        .is_empty());
    diagnostic.kind = BindingKind::Compute;
    let first = store
        .diagnostic_bindings("fixture-diagnostics", vec![diagnostic.clone()])
        .await
        .unwrap();
    assert_eq!(first[0].valid_from, at);
    diagnostic.valid_from = at + 60;
    diagnostic.observed_at = at + 60;
    let second = store
        .diagnostic_bindings("fixture-diagnostics", vec![diagnostic.clone()])
        .await
        .unwrap();
    assert_eq!(second[0].valid_from, at);
    diagnostic.runtime_container_id = Some("containerd://replacement".into());
    diagnostic.restart_count = Some(1);
    diagnostic.valid_from = at + 90;
    diagnostic.observed_at = at + 90;
    let restarted = store
        .diagnostic_bindings("fixture-diagnostics", vec![diagnostic.clone()])
        .await
        .unwrap();
    assert_eq!(restarted[0].valid_from, at + 90);
    diagnostic.owner_subject = "unattributed".into();
    diagnostic.valid_from = at + 120;
    diagnostic.observed_at = at + 120;
    let assigned = store
        .diagnostic_bindings("fixture-diagnostics", vec![diagnostic])
        .await
        .unwrap();
    assert_eq!(assigned[0].valid_from, at + 120);
    store.db.drop().await.unwrap();
}
