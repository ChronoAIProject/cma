use super::*;
#[test]
fn batching_is_default_off_strict_and_requires_snapshot_sessions() {
    for snapshot in [false, true] {
        for value in [
            None,
            Some("false"),
            Some("true"),
            Some("TRUE"),
            Some("1"),
            Some(" true "),
            Some(""),
        ] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok(match key {
                    "CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED" => Some(snapshot.to_string()),
                    "CHRONO_SANDBOX_BATCHED_READS_ENABLED" => value.map(str::to_owned),
                    _ => None,
                })
            });
            match value {
                None | Some("false") => assert!(!flags.unwrap().sandbox_batched_reads),
                Some("true") if snapshot => assert!(flags.unwrap().sandbox_batched_reads),
                _ => assert!(flags.is_err()),
            }
        }
    }
}
