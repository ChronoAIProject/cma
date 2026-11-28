//! Public owner routing reuses the serving operation's domain authorization.
use super::{ingress, relay_diagnostics::RelayDiagnostic, Refusal, Relay};
use axum::{
    extract::Request,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use layer2_core::{
    resource_access::{
        AccessPurpose, CallerContext, ResourceAccessService, ResourceCapability, ResourceTarget,
    },
    sandbox::{
        agent_attachments::AgentAttachmentService,
        agent_filesystem::{AgentFileService, FileWriteTarget},
        AgentReadService,
    },
    AgentId,
};
use std::{sync::Arc, time::SystemTime};

pub(super) fn authorize_public<'a>(
    relay: &'a Relay,
    id: &'a AgentId,
    request: &'a Request,
    diagnostic: Option<&'a RelayDiagnostic>,
    browser_proof: Option<&'a crate::auth::relay::BrowserRelayProof>,
    capture_write_target: bool,
) -> impl std::future::Future<
    Output = Result<Option<layer2_core::sandbox::agent_filesystem::FileWriteTarget>, Response>,
> + use<'a> {
    let access = relay.public_access.get();
    let context = request
        .extensions()
        .get::<layer2_core::resource_access::CallerContext>();
    let credential = request
        .extensions()
        .get::<layer1_public::DelegatedCredential>()
        .map(layer1_public::DelegatedCredential::value);
    let input = context.map(|context| PublicRequest {
        id,
        method: request.method(),
        uri: request.uri(),
        headers: ingress::headers(request),
        context,
        credential,
        browser_proof,
        peer_browser: request
            .extensions()
            .get::<crate::auth::relay::PeerBrowserReceipt>(),
        capture_write_target,
    });
    async move {
        let access = access.ok_or_else(|| {
            super::http::unavailable_with(diagnostic, Refusal::AuthorizationUnavailable)
        })?;
        let input = input.ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
        access.authorize(input).await.map_err(refusal)
    }
}

pub(super) struct PublicRequest<'a> {
    pub id: &'a AgentId,
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub headers: &'a HeaderMap,
    pub context: &'a CallerContext,
    pub credential: Option<&'a str>,
    pub browser_proof: Option<&'a crate::auth::relay::BrowserRelayProof>,
    pub peer_browser: Option<&'a crate::auth::relay::PeerBrowserReceipt>,
    pub capture_write_target: bool,
}

#[async_trait::async_trait]
pub(super) trait PublicAuthorization: Send + Sync {
    async fn authorize(
        &self,
        request: PublicRequest<'_>,
    ) -> Result<Option<FileWriteTarget>, PublicRefusal>;

    async fn project_browser(
        &self,
        _proof: &crate::auth::relay::BrowserRelayProof,
        _method: &Method,
        _uri: &Uri,
    ) -> Result<crate::auth::relay::PeerBrowserClaim, StatusCode> {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

#[derive(Debug)]
pub(super) enum PublicRefusal {
    Status(StatusCode),
    AccountDeletionInProgress,
}
impl From<StatusCode> for PublicRefusal {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

pub(crate) struct PublicAccess {
    auth: Arc<crate::auth::AuthState>,
    reads: Arc<AgentReadService>,
    access: Arc<ResourceAccessService>,
    files: Arc<AgentFileService>,
    attachments: Arc<AgentAttachmentService>,
}
impl PublicAccess {
    pub(crate) fn new(
        auth: Arc<crate::auth::AuthState>,
        reads: Arc<AgentReadService>,
        access: Arc<ResourceAccessService>,
        adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    ) -> Arc<Self> {
        Arc::new(Self {
            auth,
            reads,
            access: access.clone(),
            files: AgentFileService::new(access.clone(), adapter.clone()),
            attachments: AgentAttachmentService::new(access, adapter),
        })
    }
}

#[async_trait::async_trait]
impl PublicAuthorization for PublicAccess {
    async fn authorize(
        &self,
        request: PublicRequest<'_>,
    ) -> Result<Option<FileWriteTarget>, PublicRefusal> {
        let PublicRequest {
            id,
            method,
            uri,
            headers,
            context,
            credential,
            browser_proof,
            peer_browser,
            capture_write_target,
        } = request;
        let operation = operation(method, uri)?;
        if browser_proof.is_some_and(|proof| !proof.matches(method, uri))
            || peer_browser.is_some_and(|proof| !proof.matches(method, uri))
        {
            return Err(StatusCode::UNAUTHORIZED.into());
        }
        let caller = crate::auth::relay::RelayCaller::verify(
            self.auth.clone(),
            headers,
            context,
            credential,
            browser_proof,
            peer_browser,
        )
        .await?;
        let mut write_target = None;
        match operation {
            Operation::Preferences | Operation::Settings => {
                if !context.validate_principal()
                    || !context
                        .caller
                        .authorization
                        .allows(layer2_core::Capability::UseOwnSandboxes)
                {
                    return Err(StatusCode::FORBIDDEN.into());
                }
                let target = ResourceTarget::Agent {
                    agent_id: id.clone(),
                };
                let authorized = self
                    .access
                    .authorize(
                        context,
                        credential,
                        &target,
                        ResourceCapability::OperateAgent,
                        AccessPurpose::Mutation {
                            started_at: SystemTime::now(),
                        },
                    )
                    .await
                    .map_err(access_error)?;
                self.access
                    .validate_read_result(&authorized, &target, ResourceCapability::OperateAgent)
                    .await
                    .map_err(access_error)?;
            }
            Operation::Models => {
                let read = self
                    .reads
                    .configuration_authorized(context, credential, id)
                    .await
                    .map_err(read_error)?;
                self.reads
                    .validate_configuration_read(&read)
                    .await
                    .map_err(read_error)?;
            }
            Operation::Skills => {
                self.reads
                    .lineage(&context.caller, id)
                    .await
                    .map_err(read_error)?;
            }
            Operation::Recovery => {
                self.reads
                    .recovery_lineage(&context.caller, id)
                    .await
                    .map_err(read_error)?;
            }
            Operation::Attachment => {
                self.attachments
                    .authorize_runtime_upload(context, credential, id)
                    .await
                    .map_err(|error| match error {
                        layer2_core::sandbox::agent_attachments::AttachmentError::Access(error) => {
                            access_error(error)
                        }
                        _ => StatusCode::SERVICE_UNAVAILABLE.into(),
                    })?;
            }
            Operation::Files(capability, mutation) => {
                let purpose = if mutation {
                    AccessPurpose::Mutation {
                        started_at: SystemTime::now(),
                    }
                } else {
                    AccessPurpose::Read
                };
                write_target = self
                    .files
                    .authorize_runtime_operation(
                        context,
                        credential,
                        id,
                        capability,
                        purpose,
                        capture_write_target,
                    )
                    .await
                    .map_err(|error| match error {
                        layer2_core::sandbox::agent_filesystem::AgentFileError::Access(error) => {
                            access_error(error)
                        }
                        _ => StatusCode::SERVICE_UNAVAILABLE.into(),
                    })?;
            }
        }
        caller.current().await.map_err(PublicRefusal::from)?;
        Ok(write_target)
    }

    async fn project_browser(
        &self,
        proof: &crate::auth::relay::BrowserRelayProof,
        method: &Method,
        uri: &Uri,
    ) -> Result<crate::auth::relay::PeerBrowserClaim, StatusCode> {
        proof.peer_claim(&self.auth, method, uri).await
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Operation {
    Preferences,
    Settings,
    Models,
    Skills,
    Recovery,
    Attachment,
    Files(ResourceCapability, bool),
}
fn operation(method: &Method, uri: &Uri) -> Result<Operation, StatusCode> {
    use ResourceCapability::*;
    let suffix = uri
        .path()
        .strip_prefix("/api/v1/agents/")
        .and_then(|path| path.split_once('/'))
        .map(|(_, suffix)| suffix)
        .ok_or(StatusCode::FORBIDDEN)?;
    let read = method == Method::GET || method == Method::HEAD;
    Ok(match suffix {
        "preferences" if method == Method::PATCH => Operation::Preferences,
        "settings" if method == Method::PATCH => Operation::Settings,
        "models" if read => Operation::Models,
        "skills" if read || method == Method::POST => Operation::Skills,
        "recover-worktree" if method == Method::POST => Operation::Recovery,
        "attachments" if method == Method::POST => Operation::Attachment,
        "environment" if read => Operation::Files(ViewDetails, false),
        "files/tree" | "files/content" | "files/download" if read => {
            Operation::Files(ViewFiles, false)
        }
        "files" if method == Method::POST || method == Method::DELETE => {
            Operation::Files(OperateAgent, true)
        }
        "browser" if read || method == Method::DELETE => Operation::Files(ViewHistory, false),
        "browser" if method == Method::PUT => Operation::Files(ManageSharing, true),
        "browser/control" if read => {
            let has_operation =
                url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                    .any(|(key, _)| key == "operation_id");
            Operation::Files(
                if has_operation {
                    ManageSharing
                } else {
                    ViewHistory
                },
                false,
            )
        }
        "browser/control" if method == Method::POST => Operation::Files(ManageSharing, true),
        _ => return Err(StatusCode::METHOD_NOT_ALLOWED),
    })
}

fn access_error(error: layer2_core::resource_access::ResourceAccessError) -> PublicRefusal {
    use layer2_core::resource_access::{
        AccessError, MembershipError, ResourceAccessError::*, ResourceAccessStoreError,
    };
    let status = match error {
        AccountDeletionInProgress => return PublicRefusal::AccountDeletionInProgress,
        Store(ResourceAccessStoreError::ShareRecipientUnavailable) => StatusCode::NOT_FOUND,
        Access(AccessError::NotFound) | Store(ResourceAccessStoreError::NotFound) => {
            StatusCode::NOT_FOUND
        }
        Access(AccessError::Forbidden) | Membership(MembershipError::NotMember) => {
            StatusCode::FORBIDDEN
        }
        Access(AccessError::UnsafeInteractiveShare)
        | Store(
            ResourceAccessStoreError::UnsafeInteractiveShare
            | ResourceAccessStoreError::Conflict
            | ResourceAccessStoreError::IdempotencyConflict,
        ) => StatusCode::CONFLICT,
        Store(ResourceAccessStoreError::Invalid) => StatusCode::BAD_REQUEST,
        Store(ResourceAccessStoreError::Capacity) => StatusCode::TOO_MANY_REQUESTS,
        Access(AccessError::RevalidationRequired)
        | Membership(_)
        | Store(ResourceAccessStoreError::Unavailable | ResourceAccessStoreError::Corrupt) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
    };
    status.into()
}

pub(super) fn refusal(error: PublicRefusal) -> axum::response::Response {
    use axum::response::IntoResponse;
    let status = match error {
        PublicRefusal::Status(status) => status,
        PublicRefusal::AccountDeletionInProgress => {
            let mut response = layer1_public::rest::account_deletion_problem(
                layer2_core::account_deletion::AccountDeletionError::InProgress,
            );
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("private, no-store"),
            );
            return response;
        }
    };
    let code = match status {
        StatusCode::UNAUTHORIZED => "authentication_required",
        StatusCode::FORBIDDEN => "resource_forbidden",
        StatusCode::NOT_FOUND => "resource_not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        StatusCode::CONFLICT => "resource_access_changed",
        StatusCode::BAD_REQUEST => "invalid_resource_operation",
        StatusCode::TOO_MANY_REQUESTS => "share_capacity",
        _ => "resource_access_unavailable",
    };
    (status, [(axum::http::header::CACHE_CONTROL, "private, no-store"),
        (axum::http::header::CONTENT_TYPE, "application/problem+json")],
        axum::Json(serde_json::json!({"type":"about:blank", "title":"Agent access unavailable",
            "status":status.as_u16(), "code":code, "detail":"Current authority does not permit routing this Agent operation."}))).into_response()
}

fn read_error(error: layer2_core::sandbox::AgentReadError) -> PublicRefusal {
    use layer2_core::sandbox::AgentReadError;
    let status = match error {
        AgentReadError::Access(
            layer2_core::resource_access::ResourceAccessError::AccountDeletionInProgress,
        ) => return PublicRefusal::AccountDeletionInProgress,
        AgentReadError::NotFound => StatusCode::NOT_FOUND,
        AgentReadError::Forbidden => StatusCode::FORBIDDEN,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    status.into()
}

#[cfg(test)]
mod browser_tests;
#[cfg(test)]
mod custody_tests;
#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod standing_tests;
#[cfg(test)]
mod tests;
