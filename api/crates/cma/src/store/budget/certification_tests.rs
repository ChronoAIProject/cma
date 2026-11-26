use super::*;
use crate::store::sandbox_mongo::{is_duplicate_key, start_transaction};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::Client;

const KEY: &str = "turn:agent:2";

fn legacy_row() -> Document {
    doc! {"_id": KEY, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": {
        "owner": "owner", "agent_id": "agent", "turn_sequence": 2_i64,
        "runtime_generation": 3_i64, "period_start": 1_i64, "period_shard": 0_i32,
        "reservation": {
            "policy": {"owner_day_limit": 1000_i64, "turn_limit": 100_i64,
                "max_calls_per_turn": 5_i64, "day_timezone":"UTC", "input_token_units":1_i64,
                "output_token_units":2_i64, "max_unobserved_units":10_i64, "usage_timeout_seconds":60_i64},
            "reserved_units": 100_i64, "observed": 7_i64, "last_usage_at": 86399_i64,
            "final_usage_known": false, "stop": null, "settled": false
        }
    }}
}

fn with_outcome(mut row: Document, outcome: Document) -> Document {
    row.get_document_mut("record").unwrap().insert("certification", doc! {
        "version": 1, "receipt": {"producer":"fixture-producer", "id":"fixture-receipt",
            "digest":"a".repeat(64), "owner":"owner", "agent_id":"agent", "turn_sequence":2_i64,
            "runtime_generation":3_i64, "input_tokens":4_i64, "output_tokens":3_i64, "model_calls":1_i64},
        "outcome": outcome
    });
    row
}

fn reservation(row: &mut Document) -> &mut Document {
    row.get_document_mut("record")
        .unwrap()
        .get_document_mut("reservation")
        .unwrap()
}

#[test]
fn certification_reader_validates_retained_state_and_original_booking_day() {
    let legacy = legacy_row();
    let decoded: TurnRecord = decode(&legacy).unwrap();
    assert_eq!(
        mongodb::bson::serialize_to_document(&decoded).unwrap(),
        *legacy.get_document("record").unwrap()
    );
    let pending = with_outcome(legacy.clone(), doc! {"state": "pending_terminal"});
    assert!(certification::read(&pending).is_ok());
    let mut conflict = with_outcome(legacy.clone(), doc! {"state": "refused_conflict"});
    reservation(&mut conflict).insert("observed", 11_i64);
    assert!(certification::read(&conflict).is_ok());
    let mut applied = with_outcome(
        legacy,
        doc! {"state": "applied",
            "debit": {"at":86400_i64, "owner_day":1_i64, "units":100_i64}, "correction":-90_i64
        },
    );
    reservation(&mut applied).extend(doc! {
        "settled":true, "final_usage_known":true, "observed":10_i64, "final_model_calls":1_i64
    });
    for row in [&pending, &conflict, &applied] {
        assert!(certification::read(row).is_ok());
        assert!(decode::<TurnRecord>(row).is_err());
    }
}

#[test]
fn certification_reader_rejects_unknown_and_conflicting_receipts() {
    let pending = with_outcome(legacy_row(), doc! {"state": "pending_terminal"});
    for alteration in [
        doc! {"version": 2},
        doc! {"unexpected": true},
        doc! {"receipt": {"id":"incomplete"}},
        doc! {"outcome": {"state":"unsupported"}},
        doc! {"outcome": {"state":"pending_terminal", "debit":{}}},
    ] {
        let mut invalid = pending.clone();
        invalid
            .get_document_mut("record")
            .unwrap()
            .get_document_mut("certification")
            .unwrap()
            .extend(alteration);
        assert!(certification::read(&invalid).is_err(), "{invalid:?}");
    }
    let mut invalid = pending;
    invalid.insert("_id", "turn:agent:5");
    assert!(certification::read(&invalid).is_err());
    invalid
        .get_document_mut("record")
        .unwrap()
        .insert("certification", mongodb::bson::Bson::Null);
    assert!(certification::read(&invalid).is_err());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn certification_blocks_existing_whole_record_replacement() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_cert_reader_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let held = legacy_row();
    let stale: TurnRecord = decode(&held).unwrap();
    let pending = with_outcome(held, doc! {"state": "pending_terminal"});
    store.model_budget.insert_one(&pending).await.unwrap();

    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let error = save(&store, KEY, &stale, &mut session).await.unwrap_err();
    assert!(matches!(error, ApplyError::Driver(error) if is_duplicate_key(&error)));
    let _ = session.abort_guarded().await;

    let retained = store
        .model_budget
        .find_one(doc! {"_id":KEY})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained, pending);
    let decoded = certification::read(&retained).unwrap();
    assert!(matches!(
        save(&store, "turn:agent:4", &decoded, &mut session).await,
        Err(ApplyError::Domain(
            layer2_core::sandbox::store::SandboxStoreError::Budget(BudgetError::Unavailable)
        ))
    ));
    assert_eq!(
        store.model_budget.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
