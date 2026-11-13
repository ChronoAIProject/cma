use super::*;
use crate::rest::{
    agent_filesystem::FileProblem, instance_problem::InstanceProblem,
    resource_access_problem::ResourceProblem,
};
use axum::{body::to_bytes, response::IntoResponse};
use layer2_core::sandbox::agent_filesystem::AgentFileError;
use layer3_adapter::AdapterError;

#[tokio::test]
async fn typed_access_failures_keep_dependency_and_response_contract() {
    use ResourceAccessError::*;
    for (error, dependency, reason, code) in [
        (
            Store(ResourceAccessStoreError::Unavailable),
            Dependency::Storage,
            Reason::Unavailable,
            "resource_access_unavailable",
        ),
        (
            Store(ResourceAccessStoreError::Corrupt),
            Dependency::Storage,
            Reason::Corrupt,
            "resource_access_unavailable",
        ),
        (
            Membership(MembershipError::Unavailable),
            Dependency::IdentityProvider,
            Reason::Unavailable,
            "organization_authority_unavailable",
        ),
        (
            Membership(MembershipError::InvalidEvidence),
            Dependency::IdentityProvider,
            Reason::InvalidEvidence,
            "organization_authority_unavailable",
        ),
        (
            Access(AccessError::RevalidationRequired),
            Dependency::AuthenticationAuthority,
            Reason::AuthorityChanged,
            "organization_authority_unavailable",
        ),
    ] {
        for model in [false, true] {
            let (response, failure) = diagnostics::capture(async {
                if model {
                    InstanceProblem::from(AgentReadError::Access(error.clone())).into_response()
                } else {
                    FileProblem::from(AgentFileError::Access(error.clone())).into_response()
                }
            })
            .await;
            assert_eq!(failure, Some(Failure::new(dependency, reason)));
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(body["code"], code);
        }
    }
}

#[tokio::test]
async fn runtime_file_failures_keep_their_class_without_detail() {
    let private = "private fixture error";
    for (error, reason) in [
        (
            AdapterError::Unavailable(private.into()),
            Reason::Unavailable,
        ),
        (AdapterError::AtCapacity(private.into()), Reason::Capacity),
        (
            AdapterError::RuntimeLedgerHold(private.into()),
            Reason::RuntimeLedgerHold,
        ),
        (
            AdapterError::InputAuthorityUnavailable(private.into()),
            Reason::InputAuthority,
        ),
        (
            AdapterError::PromptNotWritten(private.into()),
            Reason::PromptNotWritten,
        ),
    ] {
        let (response, failure) = diagnostics::capture(async {
            FileProblem::from(AgentFileError::from(error)).into_response()
        })
        .await;
        assert_eq!(failure, Some(Failure::new(Dependency::Runtime, reason)));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(private));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "file_unavailable"
        );
    }
}

#[tokio::test]
async fn refused_access_and_unsupported_files_are_not_dependency_failures() {
    for error in [
        ResourceAccessError::Access(AccessError::NotFound),
        ResourceAccessError::Membership(MembershipError::NotMember),
    ] {
        let (response, failure) =
            diagnostics::capture(async { ResourceProblem(error).into_response() }).await;
        assert_ne!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(failure, None);
    }
    let (response, failure) = diagnostics::capture(async {
        FileProblem::from(AgentFileError::from(AdapterError::Unsupported(
            "private fixture".into(),
        )))
        .into_response()
    })
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(failure, None);
    for status in [
        StatusCode::OK,
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::GATEWAY_TIMEOUT,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let response = observe(Operation::FileContent, async {
            (status, "unchanged").into_response()
        })
        .await;
        assert_eq!(response.status(), status);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap(),
            "unchanged"
        );
    }
}
