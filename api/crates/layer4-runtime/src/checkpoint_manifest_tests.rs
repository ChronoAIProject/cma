use super::*;

#[test]
fn large_home_metadata_is_valid_without_creating_a_large_filesystem_fixture() {
    let manifest = Manifest {
        version: FORMAT_VERSION,
        entries: (0..346_243)
            .map(|index| directory_entry(format!("directory-{index:06}"), 0o700))
            .collect(),
    };
    validate(&manifest, Path::new("/home/cma"), 0).unwrap();
}

#[test]
fn metadata_entry_limit_still_refuses_oversized_manifests() {
    let manifest = Manifest {
        version: FORMAT_VERSION,
        entries: vec![directory_entry("entry".into(), 0o700); MAX_ENTRIES + 1],
    };
    let error = validate(&manifest, Path::new("/home/cma"), 0).unwrap_err();
    assert_eq!(error.code, "checkpoint_invalid");
    assert!(error.detail.contains("entry count"));
}

#[test]
fn increased_metadata_budget_does_not_expand_the_payload_budget() {
    assert_eq!(MAX_PAYLOAD_BYTES, 16 * 1024 * 1024 * 1024);
    let manifest = Manifest {
        version: FORMAT_VERSION,
        entries: vec![ManifestEntry {
            path: "oversized".into(),
            kind: EntryKind::File,
            mode: 0o600,
            size: MAX_PAYLOAD_BYTES + 1,
            sha256: Some("0".repeat(64)),
            target: None,
            offset: Some(0),
        }],
    };
    let error = validate(&manifest, Path::new("/home/cma"), MAX_PAYLOAD_BYTES + 1).unwrap_err();
    assert_eq!(error.code, "checkpoint_invalid");
    assert!(error.detail.contains("payload length"));
}
