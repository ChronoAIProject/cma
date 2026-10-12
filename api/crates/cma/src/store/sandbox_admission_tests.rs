use std::time::Duration;

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::doc;

use super::{admission_retry_delay, origin_filter, suffixed, unique_origin_replay};

#[test]
fn retry_delay_is_exponential_and_bounded() {
    let delays = (1..=8).map(admission_retry_delay).collect::<Vec<_>>();
    assert_eq!(
        delays,
        [1, 2, 4, 8, 16, 32, 32, 32].map(Duration::from_millis)
    );
    assert_eq!(admission_retry_delay(usize::MAX), Duration::from_millis(32));
}

#[test]
fn allocated_suffixes_preserve_the_name_limit_and_utf8() {
    assert_eq!(suffixed("research", 1), "research");
    assert_eq!(suffixed("research", 2), "research-2");
    let value = suffixed(&"é".repeat(32), 12);
    assert!(value.len() <= 64);
    assert!(value.ends_with("-12"));
}

#[test]
fn replay_discovery_covers_query_and_embedded_origin_fields() {
    assert_eq!(
        origin_filter(
            "owner",
            &layer2_core::sandbox::SandboxOrigin::Trigger {
                launch_id: "launch-one".into()
            }
        )
        .unwrap(),
        doc! {
            "$or": [
                {
                    "owner": "owner",
                    "origin_type": "trigger",
                    "origin_launch_id": "launch-one",
                },
                {
                    "record.owner": "owner",
                    "record.origin.type": "trigger",
                    "record.origin.launch_id": "launch-one",
                },
            ],
        }
    );
}

#[test]
fn replay_discovery_rejects_multiple_candidates_before_decoding() {
    assert!(matches!(
        unique_origin_replay(vec![doc! {}, doc! {}]),
        Err(SandboxStoreError::Corrupt(_))
    ));
}
