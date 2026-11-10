//! Measured comparator for #1591's public reader before the consumer lands.
//! Its projected head read and shared full scanner match provider 03fb3984a;
//! the scanner itself is reused, including its proof queries and cursor batches.
use super::*;
use crate::store::agent_log_integrity::{
    scan_complete_log_with_snapshot_session, TransactionLogError,
};
use crate::store::agent_log_telemetry::ScanCaller;
use crate::store::sandbox_mongo::{require_version, unsigned};
use mongodb::options::ReadConcern;

async fn provider_full_page(
    fixture: &Fixture,
    after: u64,
    limit: u16,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut session = fixture
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let Some(row) = fixture
        .store
        .agents
        .find_one(doc! { "_id": fixture.id.as_str() })
        .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(Vec::new());
    };
    require_version(&row).map_err(SandboxStoreError::Corrupt)?;
    let head = row
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let head = unsigned(head, "agent log sequence").map_err(SandboxStoreError::Corrupt)?;
    let mut page = Vec::new();
    scan_complete_log_with_snapshot_session(
        ScanCaller::PublicWindowPrefix,
        &fixture.store,
        &fixture.id,
        head,
        &mut session,
        |entry| {
            if entry.seq > after && page.len() < usize::from(limit) {
                page.push(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => unavailable(error),
    })?;
    Ok(page)
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_windows_reduce_checkouts_against_the_actual_provider_reader() {
    for history in [1_000, 10_000] {
        let mut fixture = Fixture::new(history).await.prepare().await;
        let window = Window::Page {
            after: history - 200,
            limit: 200,
        };
        let before = fixture.reset_commands();
        let historical = fixture.read(window, &LegacyIntegrity).await.unwrap();
        let historical_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
        for state in ["enabled", "disabled_prepared", "unprepared"] {
            if state == "disabled_prepared" {
                fixture.store = MongoSandboxStore::open(&fixture.client, &fixture.name)
                    .await
                    .unwrap();
            } else if state == "unprepared" {
                fixture
                    .store
                    .agents
                    .update_one(
                        doc! { "_id": fixture.id.as_str() },
                        doc! { "$unset": { INTEGRITY_FIELD: "" } },
                    )
                    .await
                    .unwrap();
            }
            let before = fixture.reset_commands();
            let provider_page = provider_full_page(&fixture, history - 200, 200)
                .await
                .unwrap();
            let provider_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(provider_page, historical, "{state}");
            fixture.assert_read_only();
            let before = fixture.reset_commands();
            let consumer_page = fixture
                .store
                .agent_log(&fixture.id, history - 200, 200)
                .await
                .unwrap();
            let consumer_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(consumer_page, provider_page, "{state}");
            fixture.assert_read_only();
            let (expected_provider, expected_consumer) = match state {
                "enabled" => (historical_checkouts + 3, 4),
                "disabled_prepared" => (historical_checkouts + 3, historical_checkouts + 2),
                "unprepared" => (historical_checkouts + 1, historical_checkouts),
                _ => unreachable!(),
            };
            assert_eq!(provider_checkouts, expected_provider, "{state}");
            assert_eq!(consumer_checkouts, expected_consumer, "{state}");
            assert!(consumer_checkouts < provider_checkouts, "{state}");
            eprintln!("public_window_provider_comparison history={history} page=200 state={state} provider_checkouts={provider_checkouts} consumer_checkouts={consumer_checkouts} historical_checkouts={historical_checkouts}");
        }
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_window_refuses_a_checkpoint_revoked_by_a_semantic_audit() {
    let fixture = Fixture::new(200).await.prepare().await;
    // Keep structural coordinates and the tail intact; only the explicit audit
    // can disprove this previously trusted middle row's semantics.
    fixture
        .store
        .agent_log
        .update_one(
            doc! { "agent_id": fixture.id.as_str(), "seq": 100_i64 },
            doc! { "$set": { "record.appended_at": "x".repeat(NOW.len()) } },
        )
        .await
        .unwrap();
    assert!(fixture
        .store
        .clone()
        .prepare_bounded_admission(std::slice::from_ref(&fixture.id))
        .await
        .is_err());
    let filter = doc! { "_id": fixture.id.as_str() };
    let quarantined = fixture
        .store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(quarantined.contains_key(QUARANTINE_FIELD));
    fixture.reset_commands();
    assert!(matches!(
        fixture.store.agent_log(&fixture.id, 195, 5).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    fixture.assert_read_only();
    assert_eq!(
        fixture
            .store
            .agents
            .find_one(filter)
            .await
            .unwrap()
            .unwrap(),
        quarantined
    );
    fixture.cleanup().await;
}
