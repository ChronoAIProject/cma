use super::*;

#[test]
fn failed_sampling_keeps_last_good_values_and_freshness() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        initialize("agent_commands", "queued");
        assert!(!handle.render().contains("cma_worker_durable_items"));
        sampled("agent_commands", "queued", true);
        let before = handle.render();
        sampled("agent_commands", "queued", false);
        let after = handle.render();
        assert!(after.contains(
            "cma_worker_durable_sample_success{worker=\"agent_commands\",state=\"queued\"} 0"
        ));
        let success = |text: &str| {
            text.lines()
                .find(|line| {
                    line.starts_with("cma_worker_durable_sample_success_timestamp_seconds{")
                })
                .unwrap()
                .to_owned()
        };
        assert_eq!(success(&before), success(&after));
    });
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn durable_summaries_exclude_live_leases_future_retries_and_terminals() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_observations_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let observations = Observations::new(database.clone());
    for (collection, name, keys) in [
        (
            "agent_commands",
            "agent_command_observations",
            doc! { "status": 1, "record.receipt.admitted_at": 1, "queue_deadline": 1 },
        ),
        (
            "sandbox_controls",
            "sandbox_control_observations",
            doc! { "queued": 1, "record.admitted_at": 1, "queue_deadline": 1 },
        ),
        (
            "trigger_launches",
            "trigger_launches_observations",
            doc! { "work_step": 1, "work_ready_at": 1, "work_lease_expires_at": 1, "blocked_reason": 1, "blocked_reason.type": 1 },
        ),
        (
            "agent_lifecycle_operations",
            "status_1_retry_at_1_created_at_1",
            doc! { "status": 1, "retry_at": 1, "created_at": 1 },
        ),
    ] {
        database
            .collection::<Document>(collection)
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(keys)
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .name(name.to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .unwrap();
    }
    let now = DateTime::now();
    let old = DateTime::from_millis(now.timestamp_millis() - 120_000);
    let future = DateTime::from_millis(now.timestamp_millis() + 120_000);
    let old_text = layer2_core::rfc3339_at(old.to_system_time());
    let future_text = layer2_core::rfc3339_at(future.to_system_time());
    let version = super::super::schema::DOCUMENT_SCHEMA_VERSION;
    database.collection::<Document>("agent_lifecycle_operations").insert_many([
        doc! { "status": "queued", "created_at": old, "retry_at": Bson::Null, "document_schema_version": version },
        doc! { "status": "queued", "created_at": old, "retry_at": future, "document_schema_version": version },
        doc! { "status": "running", "created_at": old, "lease_expires_at": future, "document_schema_version": version },
        doc! { "status": "running", "created_at": old, "lease_expires_at": old, "document_schema_version": version },
        doc! { "status": "waiting_for_connection", "created_at": old, "document_schema_version": version },
        doc! { "status": "succeeded", "created_at": old, "document_schema_version": version },
    ]).await.unwrap();
    database.collection::<Document>("trigger_launches").insert_many([
        doc! { "work_step": "allocate", "work_ready_at": &old_text, "blocked_reason": Bson::Null, "work_lease_expires_at": Bson::Null },
        doc! { "work_step": "none", "work_ready_at": &old_text, "blocked_reason": Bson::Null },
        doc! { "work_step": "allocate", "work_ready_at": &old_text, "blocked_reason": { "type": "user_action" } },
        doc! { "work_step": "allocate", "work_ready_at": &old_text, "blocked_reason": Bson::Null, "work_lease_expires_at": &future_text },
    ]).await.unwrap();
    database.collection::<Document>("agent_commands").insert_many([
        doc! { "status": "queued", "queue_deadline": &old_text, "record": { "receipt": { "admitted_at": &old_text } } },
        doc! { "status": "in_progress", "queue_deadline": &old_text, "record": { "receipt": { "admitted_at": &old_text } } },
        doc! { "status": "waiting_action", "queue_deadline": &old_text, "record": { "receipt": { "admitted_at": &old_text } } },
        doc! { "status": "completed", "queue_deadline": &old_text },
    ]).await.unwrap();
    for spec in specifications(now) {
        let summary = observations.read(&spec).await.unwrap();
        let expected = match (spec.worker, spec.state) {
            ("agents", _) => 2,
            ("sandbox_commands", _) => 0,
            _ => 1,
        };
        assert_eq!(summary.count, expected, "{} / {}", spec.worker, spec.state);
        if expected > 0 {
            assert!((summary.oldest - old.timestamp_millis() as f64 / 1000.).abs() < 1.);
        }
    }
    database.drop().await.unwrap();
}
