use super::*;

#[test]
fn install_request_digest_has_a_stable_golden_encoding() {
    let request = InstallRequest::bounded(
        "sandbox-install-v1:sandbox-1",
        vec!["printf one".into(), "printf two".into()],
        600,
        32_768,
    );
    assert_eq!(
        request.request_sha256,
        "3933c5d0d0ea7259b3ba39409a43093a729319c0c71bf038489c74bbe8467dc2"
    );
    assert!(request.has_valid_sha256());
}

#[test]
fn install_request_digest_covers_only_execution_fields() {
    let original = InstallRequest::bounded(
        "sandbox-install-v1:sandbox-1",
        vec!["printf one".into()],
        600,
        32_768,
    );
    let renamed = InstallRequest::bounded(
        "sandbox-install-v1:sandbox-2",
        original.commands.clone(),
        original.timeout_seconds,
        original.output_limit_bytes,
    );
    assert_eq!(renamed.request_sha256, original.request_sha256);

    let variants = [
        InstallRequest::with_timeouts(
            &original.operation_id,
            original.commands.clone(),
            original.timeout_seconds,
            original.total_timeout_seconds + 1,
            original.output_limit_bytes,
        ),
        InstallRequest::bounded(
            &original.operation_id,
            vec!["printf changed".into()],
            original.timeout_seconds,
            original.output_limit_bytes,
        ),
        InstallRequest::bounded(
            &original.operation_id,
            original.commands.clone(),
            original.timeout_seconds + 1,
            original.output_limit_bytes,
        ),
        InstallRequest::bounded(
            &original.operation_id,
            original.commands.clone(),
            original.timeout_seconds,
            original.output_limit_bytes + 1,
        ),
    ];
    assert!(variants
        .iter()
        .all(|changed| changed.request_sha256 != original.request_sha256));
}
