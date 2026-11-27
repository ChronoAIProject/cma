use super::{
    projection,
    tests::{agent, document, saved},
};
use crate::store::budget::{decode, save, ApplyError, MongoSandboxStore, TurnRecord};
use crate::store::sandbox_mongo::{is_duplicate_key, start_transaction};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::{
    bson::{doc, Bson, Document},
    Client,
};

const KEY: &str = "turn:agent:2";

fn legacy_row() -> Document {
    let mut parent = saved(&agent("agent"));
    parent.reservation.observed = 7;
    parent.reservation.last_usage_at = 86399;
    document(&parent)
}

fn fixture(outcome: Document, input: i64, settled: bool) -> Document {
    let mut row = legacy_row();
    let parent = row.get_document_mut("record").unwrap();
    let applied = outcome.get_str("state").unwrap() == "applied";
    let reserve = parent.get_document_mut("reservation").unwrap();
    reserve.insert("settled", settled);
    if applied {
        reserve.extend(
            doc! {"observed":input + 6, "final_usage_known":true, "final_model_calls":1_i64},
        );
    }
    parent.insert("certification", doc! {"version":1, "outcome":outcome,
        "receipt":{"producer":"fixture-producer", "id":"fixture-receipt", "digest":"a".repeat(64),
            "owner":parent.get_str("owner").unwrap(), "agent_id":"agent", "turn_sequence":2_i64,
            "runtime_generation":1_i64, "input_tokens":input, "output_tokens":3_i64, "model_calls":1_i64}});
    row
}

#[test]
fn inventory_validates_original_debit_source_amount_and_date() {
    for (source, units, final_units, at, day, expected) in [
        ("conservative", 100_i64, 10_i64, 86400_i64, 1_i64, true),
        ("conservative", 100, 120, 86400, 1, true),
        ("certified", 10, 10, 86400, 1, true),
        ("conservative", 5000, 10, 86400, 1, false),
        ("conservative", 0, 10, 86400, 1, false),
        ("certified", 100, 10, 86400, 1, false),
        ("certified", 120, 120, 86400, 1, false),
        ("conservative", 100, 10, 86400, 0, false),
        ("conservative", 100, 10, 86398, 0, false),
    ] {
        let row = fixture(
            doc! {"state":"applied", "correction":final_units - units,
            "debit":{"source":source,"units":units,"at":at,"owner_day":day}},
            final_units - 6,
            true,
        );
        assert_eq!(projection::turn(&row).is_ok(), expected, "{row:?}");
        assert!(decode::<TurnRecord>(&row).is_err());
    }
}

#[test]
fn inventory_validates_pending_and_conflict_states_strictly() {
    let pending = fixture(doc! {"state":"pending_terminal"}, 4, false);
    assert!(projection::turn(&pending).is_ok());
    for (settled, debit, expected) in [
        (false, None, true),
        (true, None, false),
        (true, Some("conservative"), true),
        (true, Some("certified"), false),
    ] {
        let mut outcome = doc! {"state":"refused_conflict"};
        if let Some(source) = debit {
            outcome.insert(
                "debit",
                doc! {"source":source,"units":100_i64,"at":86400_i64,"owner_day":1_i64},
            );
        }
        assert_eq!(
            projection::turn(&fixture(outcome, 0, settled)).is_ok(),
            expected
        );
    }
    for alteration in [
        doc! {"version":2},
        doc! {"unknown":true},
        doc! {"receipt":{"id":"incomplete"}},
        doc! {"outcome":{"state":"unsupported"}},
        doc! {"outcome":{"state":"pending_terminal","debit":{}}},
    ] {
        let mut invalid = pending.clone();
        let record = invalid.get_document_mut("record").unwrap();
        record
            .get_document_mut("certification")
            .unwrap()
            .extend(alteration);
        assert!(projection::turn(&invalid).is_err(), "{invalid:?}");
    }
    let mut invalid = pending;
    invalid.insert("_id", "turn:agent:5");
    assert!(projection::turn(&invalid).is_err());
    invalid.insert("_id", KEY);
    let record = invalid.get_document_mut("record").unwrap();
    record.insert("certification", Bson::Null);
    assert!(projection::turn(&invalid).is_err());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn certification_blocks_existing_whole_record_replacement() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_cert_reader_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let legacy = legacy_row();
    let stale: TurnRecord = decode(&legacy).unwrap();
    assert_eq!(
        mongodb::bson::serialize_to_document(&stale).unwrap(),
        *legacy.get_document("record").unwrap()
    );
    let pending = fixture(doc! {"state":"pending_terminal"}, 4, false);
    let rows = &store.model_budget;
    rows.insert_one(&pending).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let error = save(&store, KEY, &stale, &mut session).await.unwrap_err();
    assert!(matches!(error, ApplyError::Driver(error) if is_duplicate_key(&error)));
    let _ = session.abort_guarded().await;
    let retained = rows.find_one(doc! {"_id":KEY}).await.unwrap().unwrap();
    assert_eq!(retained, pending);
    let certified = projection::turn(&retained).unwrap();
    let error = save(&store, "turn:agent:4", &certified, &mut session)
        .await
        .unwrap_err();
    assert!(matches!(error, ApplyError::Domain(_)));
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}
