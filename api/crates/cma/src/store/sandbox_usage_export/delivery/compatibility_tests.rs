use super::*;

// Freeze the preceding reader independently of the current persistence DTOs.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousState {
    version: u8,
    destination: String,
    attempt_id: String,
    generation: i64,
    lease_until: DateTime,
    due_at: DateTime,
    status: PreviousStatus,
    reason: Option<String>,
    receipt: Option<Receipt>,
    cursor: Option<PreviousCursor>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PreviousStatus {
    Pending,
    Accepted,
    Held,
    ProducerForbidden,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousCursor {
    stream_id: String,
    contiguous_sequence: String,
    validated_sequence: String,
    highest_sequence: String,
    head_digest: Option<String>,
    missing_ranges: Vec<PreviousRange>,
    snapshot: String,
    next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousRange {
    from: String,
    through: String,
}

pub(in crate::store::sandbox_usage_export) fn previous_roundtrip(value: Bson) -> Bson {
    let state: PreviousState = mongodb::bson::deserialize_from_bson(value).unwrap();
    mongodb::bson::serialize_to_bson(&state).unwrap()
}

fn state_fixture(cursor: Bson) -> Bson {
    Bson::Document(doc! {
        "version":1_i32,"destination":"a".repeat(64),
        "attempt_id":uuid::Uuid::new_v4().to_string(),"generation":1_i64,
        "lease_until":DateTime::from_millis(0),"due_at":DateTime::from_millis(0),
        "status":"pending","reason":Bson::Null,"receipt":Bson::Null,"cursor":cursor,
    })
}

fn prior_cursor() -> Document {
    doc! {
        "stream_id":uuid::Uuid::new_v4().to_string(),"contiguous_sequence":"1",
        "validated_sequence":"1","highest_sequence":"3","head_digest":"a".repeat(64),
        "missing_ranges":[{"from":"2","through":"2"}],
        "snapshot":"previous-server-observation","next_cursor":"previous-page",
    }
}

#[test]
fn delivery_v1_retains_prior_observations_in_both_reader_directions() {
    for cursor in [Bson::Document(prior_cursor()), Bson::Null] {
        let old = previous_roundtrip(state_fixture(cursor));
        let current: State = mongodb::bson::deserialize_from_bson(old.clone()).unwrap();
        let written = mongodb::bson::serialize_to_bson(&current).unwrap();
        assert_eq!(written, old);
        let rollback = previous_roundtrip(written);
        let reread: State = mongodb::bson::deserialize_from_bson(rollback.clone()).unwrap();
        assert_eq!(mongodb::bson::serialize_to_bson(&reread).unwrap(), rollback);
    }
}

#[test]
fn previous_reader_controls_reject_unversioned_runtime_cursor_fields() {
    let mut added = prior_cursor();
    added.insert("validated_head_digest", "a".repeat(64));
    let mut omitted = prior_cursor();
    omitted.remove("snapshot");
    for cursor in [added, omitted] {
        assert!(
            mongodb::bson::deserialize_from_bson::<PreviousState>(state_fixture(Bson::Document(
                cursor
            )))
            .is_err()
        );
    }
}
