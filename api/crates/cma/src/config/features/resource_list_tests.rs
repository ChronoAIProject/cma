use super::*;

#[test]
fn resource_list_batching_is_independent_default_off_and_strict() {
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
            Ok((key == "CHRONO_RESOURCE_LIST_BATCH_READS_ENABLED")
                .then_some(value)
                .flatten()
                .map(str::to_owned))
        });
        match value {
            None | Some("false") => assert!(!flags.unwrap().resource_list_batch_reads),
            Some("true") => {
                let flags = flags.unwrap();
                assert!(flags.resource_list_batch_reads);
                assert!(!flags.access_snapshot_session && !flags.sandbox_batched_reads);
            }
            _ => assert!(flags.is_err()),
        }
    }
}
