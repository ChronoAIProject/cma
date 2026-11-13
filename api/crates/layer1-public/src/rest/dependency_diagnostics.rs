//! Attribute only completed 503s on the two dependency-backed read routes.
use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use layer2_core::{
    dependency_diagnostics::{self as diagnostics, Dependency, Failure, Operation, Reason},
    resource_access::{
        AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
    },
    sandbox::AgentReadError,
};

pub(super) async fn file_content(request: Request, next: Next) -> Response {
    observe(Operation::FileContent, next.run(request)).await
}
pub(super) async fn model_list(request: Request, next: Next) -> Response {
    observe(Operation::ModelList, next.run(request)).await
}

async fn observe(
    operation: Operation,
    future: impl std::future::Future<Output = Response>,
) -> Response {
    let (response, failure) = diagnostics::capture(future).await;
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        diagnostics::report(operation, failure);
    }
    response
}

pub(super) fn resource_failure(error: &ResourceAccessError) {
    use ResourceAccessError::*;
    let failure = match error {
        Store(ResourceAccessStoreError::Unavailable) => {
            Failure::new(Dependency::Storage, Reason::Unavailable)
        }
        Store(ResourceAccessStoreError::Corrupt) => {
            Failure::new(Dependency::Storage, Reason::Corrupt)
        }
        Membership(MembershipError::Unavailable) => {
            Failure::new(Dependency::IdentityProvider, Reason::Unavailable)
        }
        Membership(MembershipError::InvalidEvidence) => {
            Failure::new(Dependency::IdentityProvider, Reason::InvalidEvidence)
        }
        Access(AccessError::RevalidationRequired) => Failure::new(
            Dependency::AuthenticationAuthority,
            Reason::AuthorityChanged,
        ),
        _ => return,
    };
    failure.record();
}

pub(super) fn read_failure(error: &AgentReadError) {
    let failure = match error {
        AgentReadError::Access(error) => return resource_failure(error),
        AgentReadError::Corrupt(_) => Failure::new(Dependency::Storage, Reason::Corrupt),
        // Store mappings record first; absent service composition is local.
        AgentReadError::Unavailable(_) => Failure::new(Dependency::Local, Reason::Unavailable),
        _ => return,
    };
    failure.record();
}

#[cfg(test)]
mod tests;
