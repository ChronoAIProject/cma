use super::deliverables::failure;
use layer2_core::{
    document_artifacts::DocumentError,
    resource_access::{
        AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
    },
    sandbox::{
        agent_filesystem::AgentFileError, answer_deliverables::DeliverableError, AgentReadError,
        RuntimeCapacityError,
    },
};
use serde_json::json;

#[test]
fn deliverable_errors_distinguish_repair_from_transient_retry() {
    use AgentFileError as File;
    use DeliverableError as Delivery;
    let cases = [
        (Delivery::Invalid, "invalid_resource", false),
        (Delivery::File(File::TooLarge), "too_large", false),
        (Delivery::Changed, "resource_changed", false),
        (Delivery::File(File::Conflict), "resource_changed", false),
        (Delivery::File(File::NotReady), "runtime_not_ready", true),
        (
            Delivery::File(File::Unavailable),
            "runtime_unavailable",
            true,
        ),
        (
            Delivery::File(File::Unsupported),
            "runtime_unsupported",
            false,
        ),
        (
            Delivery::File(File::RuntimeCapacity(RuntimeCapacityError::Request)),
            "runtime_busy",
            true,
        ),
        (
            Delivery::Answer(AgentReadError::RuntimeCapacity(
                RuntimeCapacityError::Stream,
            )),
            "runtime_busy",
            true,
        ),
        (Delivery::Unavailable, "verification_unavailable", false),
        (
            Delivery::Answer(AgentReadError::Unavailable("private diagnostic".into())),
            "verification_unavailable",
            true,
        ),
        (
            Delivery::Artifact(ResourceAccessError::Store(
                ResourceAccessStoreError::Unavailable,
            )),
            "verification_unavailable",
            true,
        ),
        (
            Delivery::File(File::Access(ResourceAccessError::Membership(
                MembershipError::Unavailable,
            ))),
            "verification_unavailable",
            true,
        ),
        (
            Delivery::Document(DocumentError::Unavailable),
            "verification_unavailable",
            true,
        ),
        (
            Delivery::Document(DocumentError::Corrupt),
            "verification_failed",
            false,
        ),
    ];
    for (error, code, retryable) in cases {
        let response = failure(error);
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"],
            json!({"error":{"code":code, "retryable":retryable}})
        );
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(code), "{response}");
        assert!(text.contains("Do not invent a reference or claim publication."));
        assert!(!response.to_string().contains("<!--cma-deliverable:"));
    }
}

#[test]
fn deliverable_errors_conceal_missing_denied_and_stale_authority_identically() {
    use AgentReadError as Read;
    use DeliverableError as Delivery;
    let expected = failure(Delivery::NotFound);
    assert_eq!(
        expected["structuredContent"]["error"],
        json!({"code":"not_accessible", "retryable":false})
    );
    for error in [
        Delivery::File(AgentFileError::Invalid),
        Delivery::File(AgentFileError::NotFound),
        Delivery::Answer(Read::NotFound),
        Delivery::Answer(Read::Forbidden),
        Delivery::Document(DocumentError::NotFound),
    ] {
        assert_eq!(failure(error), expected);
    }
    for error in [
        ResourceAccessError::Access(AccessError::NotFound),
        ResourceAccessError::Access(AccessError::Forbidden),
        ResourceAccessError::Access(AccessError::RevalidationRequired),
        ResourceAccessError::Access(AccessError::UnsafeInteractiveShare),
        ResourceAccessError::Store(ResourceAccessStoreError::NotFound),
        ResourceAccessError::Store(ResourceAccessStoreError::Conflict),
        ResourceAccessError::Membership(MembershipError::NotMember),
        ResourceAccessError::Membership(MembershipError::InvalidEvidence),
    ] {
        for nested in [
            Delivery::Artifact(error.clone()),
            Delivery::File(AgentFileError::Access(error.clone())),
            Delivery::Answer(Read::Access(error)),
        ] {
            assert_eq!(failure(nested), expected);
        }
    }
}

#[test]
fn deliverable_errors_never_echo_internal_diagnostics() {
    let detail = "/private/example/report.xlsx capability-material upstream-response-body";
    for error in [
        AgentReadError::Unavailable(detail.into()),
        AgentReadError::Corrupt(detail.into()),
        AgentReadError::ResourcesUnsupported(detail.into()),
    ] {
        let response = failure(DeliverableError::Answer(error));
        let encoded = response.to_string();
        for sensitive in detail.split(' ') {
            assert!(!encoded.contains(sensitive), "{encoded}");
        }
        assert!(response["structuredContent"].get("reference").is_none());
        assert!(response["structuredContent"].get("marker").is_none());
    }
    let fallback = failure(DeliverableError::Answer(AgentReadError::Corrupt(
        detail.into(),
    )));
    assert_eq!(
        fallback["structuredContent"]["error"],
        json!({"code":"verification_failed", "retryable":false})
    );
}
