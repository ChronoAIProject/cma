use super::*;

pub(super) async fn started_at(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    admitted_at: &str,
) -> Result<Option<String>, SandboxStoreError> {
    let base = doc! {
        "sandbox_id": sandbox_id.as_str(),
        "record.appended_at": { "$gte": admitted_at },
        "record.kind.step.kind": "run_lifecycle_operation",
    };
    let mut progress_query = base.clone();
    progress_query.insert("record.kind.type", "work_step_completed");
    let latest_progress = store
        .sandbox_events
        .find_one(progress_query)
        .sort(doc! { "seq": -1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let after = latest_progress
        .as_ref()
        .map(read_event)
        .transpose()?
        .map(|event| event.seq)
        .unwrap_or(0);
    let after = signed(after, "sandbox event sequence").map_err(SandboxStoreError::Corrupt)?;
    let mut failure_query = base;
    failure_query.insert("seq", doc! { "$gt": after });
    failure_query.insert("record.kind.type", "work_step_failed");
    store
        .sandbox_events
        .find_one(failure_query)
        .sort(doc! { "seq": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_event)
        .transpose()
        .map(|event| event.map(|event| event.appended_at))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn issue_1164_mongo_reopen_deadline_reads_first_failure_after_phase_progress() {
    use layer2_core::sandbox::{SandboxWorkStep, TypedFailure};

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("reopen_deadline_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = SandboxId("sbx_reopen_deadline_fixture".into());
    let failure = || SandboxEventKind::WorkStepFailed {
        step: SandboxWorkStep::RunLifecycleOperation,
        failure: TypedFailure {
            class: "retryable_transient".into(),
            detail: "synthetic retry".into(),
        },
    };
    let events = [
        ("00", failure()),
        (
            "01",
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            },
        ),
        ("02", failure()),
        ("03", failure()),
    ];
    let documents = events
        .into_iter()
        .enumerate()
        .map(|(index, (second, kind))| {
            event_document(&SandboxEvent {
                sandbox_id: sandbox.clone(),
                seq: index as u64 + 1,
                appended_at: format!("2026-01-01T00:00:{second}Z"),
                kind,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    store.sandbox_events.insert_many(documents).await.unwrap();
    assert_eq!(
        store
            .lifecycle_retry_started_at(&sandbox, "2026-01-01T00:00:00Z")
            .await
            .unwrap()
            .as_deref(),
        Some("2026-01-01T00:00:02Z"),
    );
    client.database(&name).drop().await.unwrap();
}
