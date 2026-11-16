//! Public runtime routing reuses the serving operation's domain authorization.
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use layer2_core::{
    resource_access::{
        AccessPurpose, CallerContext, ResourceAccessService, ResourceCapability, ResourceTarget,
    },
    sandbox::{
        agent_attachments::AgentAttachmentService, agent_filesystem::AgentFileService,
        AgentReadService,
    },
    AgentId, Capability,
};
use std::{sync::Arc, time::SystemTime};

mod media;
pub(super) use media::{media_mutation, PublicMediaRequest};

pub(super) struct PublicRequest<'a> {
    pub id: &'a AgentId,
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub headers: &'a HeaderMap,
    pub context: &'a CallerContext,
    pub credential: Option<&'a str>,
}

pub(super) struct RedemptionRequest<'a> {
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub headers: &'a HeaderMap,
    pub context: &'a CallerContext,
    pub credential: Option<&'a str>,
    pub body: &'a [u8],
}

pub(super) fn redemption_path<'a>(method: &Method, uri: &'a Uri) -> Option<&'a str> {
    if method != Method::POST {
        return None;
    }
    let id = uri
        .path()
        .strip_prefix("/api/v1/agent-share-links/")?
        .strip_suffix("/redeem")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

#[async_trait::async_trait]
pub(super) trait PublicAuthorization: Send + Sync {
    async fn authorize(&self, request: PublicRequest<'_>) -> Result<(), StatusCode>;
    async fn resolve_media(&self, _request: PublicMediaRequest<'_>) -> Result<AgentId, StatusCode> {
        Err(StatusCode::FORBIDDEN)
    }
    async fn resolve_redemption(
        &self,
        _request: RedemptionRequest<'_>,
    ) -> Result<AgentId, StatusCode> {
        Err(StatusCode::FORBIDDEN)
    }
}

pub(crate) struct PublicAccess {
    auth: Arc<crate::auth::AuthState>,
    reads: Arc<AgentReadService>,
    access: Arc<ResourceAccessService>,
    files: Arc<AgentFileService>,
    attachments: Arc<AgentAttachmentService>,
    media: Option<Arc<crate::media_continuations::MediaContinuations>>,
}
impl PublicAccess {
    pub(crate) fn with_media(
        mut self: Arc<Self>,
        media: Option<Arc<crate::media_continuations::MediaContinuations>>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("media routes composed before sharing policy")
            .media = media;
        self
    }
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
            media: None,
        })
    }
}

#[async_trait::async_trait]
impl PublicAuthorization for PublicAccess {
    async fn resolve_media(&self, request: PublicMediaRequest<'_>) -> Result<AgentId, StatusCode> {
        self.media_target(request).await
    }
    async fn resolve_redemption(
        &self,
        request: RedemptionRequest<'_>,
    ) -> Result<AgentId, StatusCode> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RedemptionBody {
            token: String,
        }
        let share_id = redemption_path(request.method, request.uri).ok_or(StatusCode::FORBIDDEN)?;
        let body: RedemptionBody =
            serde_json::from_slice(request.body).map_err(|_| StatusCode::BAD_REQUEST)?;
        let caller = crate::auth::relay::RelayCaller::verify(
            self.auth.clone(),
            request.headers,
            request.context,
            request.credential,
        )
        .await?;
        let id = self
            .access
            .authorize_agent_share_redemption(
                request.context,
                request.credential,
                share_id,
                &body.token,
            )
            .await
            .map_err(access_error)?;
        caller.current().await?;
        Ok(id)
    }

    async fn authorize(&self, request: PublicRequest<'_>) -> Result<(), StatusCode> {
        let PublicRequest {
            id,
            method,
            uri,
            headers,
            context,
            credential,
        } = request;
        let operation = operation(method, uri)?;
        let caller = crate::auth::relay::RelayCaller::verify(
            self.auth.clone(),
            headers,
            context,
            credential,
        )
        .await?;
        match operation {
            Operation::Artifact | Operation::Share | Operation::PersonalShare => {
                if !context.validate_principal()
                    || (operation == Operation::Artifact
                        && !matches!(
                            &context.actor,
                            layer2_core::resource_access::ResourceActor::Human { .. }
                                | layer2_core::resource_access::ResourceActor::Api {
                                    delegation_id: None,
                                    ..
                                }
                        ))
                {
                    return Err(StatusCode::FORBIDDEN);
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
                        ResourceCapability::ManageSharing,
                        AccessPurpose::Mutation {
                            started_at: SystemTime::now(),
                        },
                    )
                    .await
                    .map_err(access_error)?;
                self.access
                    .validate_read_result(&authorized, &target, ResourceCapability::ManageSharing)
                    .await
                    .map_err(access_error)?;
                if matches!(operation, Operation::Share | Operation::PersonalShare) {
                    self.reads
                        .lineage(&context.caller, id)
                        .await
                        .map_err(read_error)?;
                }
            }
            Operation::CapacityRecovery => {
                if matches!(
                    context.credential_source,
                    layer2_core::resource_access::CredentialSource::Binding
                        | layer2_core::resource_access::CredentialSource::RequestBoundRead { .. }
                ) {
                    return Err(StatusCode::FORBIDDEN);
                }
                let target = ResourceTarget::Agent {
                    agent_id: id.clone(),
                };
                let authorized = self
                    .access
                    .authorize_personal(&context.caller, &target, ResourceCapability::OperateAgent)
                    .await
                    .map_err(access_error)?;
                self.access
                    .validate_read_result(&authorized, &target, ResourceCapability::OperateAgent)
                    .await
                    .map_err(access_error)?;
            }
            Operation::Configuration | Operation::PersonalControl => {
                // Routing is a mutation, including a cached idempotent replay.
                // A view/read credential must not gain owner execution scope.
                // The receiving handler validates the exact settings payload
                // and creates its existing transactional settings authority.
                if !context.validate_principal()
                    || !context
                        .caller
                        .authorization
                        .allows(Capability::UseOwnSandboxes)
                {
                    return Err(StatusCode::FORBIDDEN);
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
                if operation == Operation::PersonalControl {
                    // Stop/Delete retain AgentService's personal owner rule;
                    // an interactive share cannot become teardown authority.
                    self.reads
                        .lineage(&context.caller, id)
                        .await
                        .map_err(read_error)?;
                }
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
                        _ => StatusCode::SERVICE_UNAVAILABLE,
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
                self.files
                    .authorize_runtime_operation(context, credential, id, capability, purpose)
                    .await
                    .map_err(|error| match error {
                        layer2_core::sandbox::agent_filesystem::AgentFileError::Access(error) => {
                            access_error(error)
                        }
                        _ => StatusCode::SERVICE_UNAVAILABLE,
                    })?;
            }
        }
        caller.current().await
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Operation {
    Artifact,
    Share,
    PersonalShare,
    CapacityRecovery,
    Configuration,
    PersonalControl,
    Models,
    Skills,
    Recovery,
    Attachment,
    Files(ResourceCapability, bool),
}
fn operation(method: &Method, uri: &Uri) -> Result<Operation, StatusCode> {
    use ResourceCapability::*;
    if uri.path().starts_with("/api/v1/resource-shares/agent/") {
        return if matches!(*method, Method::POST | Method::PATCH | Method::DELETE) {
            Ok(Operation::Share)
        } else {
            Err(StatusCode::METHOD_NOT_ALLOWED)
        };
    }
    let suffix = uri
        .path()
        .strip_prefix("/api/v1/agents/")
        .map(|path| path.split_once('/').map_or("", |(_, suffix)| suffix))
        .ok_or(StatusCode::FORBIDDEN)?;
    let read = method == Method::GET || method == Method::HEAD;
    Ok(match suffix {
        "" if method == Method::DELETE => Operation::PersonalControl,
        "stop" if method == Method::POST => Operation::PersonalControl,
        "preferences" | "settings" if method == Method::PATCH => Operation::Configuration,
        "artifacts" if method == Method::POST => Operation::Artifact,
        "share-links" if method == Method::POST => Operation::PersonalShare,
        suffix if suffix.starts_with("share-links/") && method == Method::DELETE => {
            Operation::PersonalShare
        }
        "capacity-recovery/admit" | "capacity-recovery/resume" if method == Method::POST => {
            Operation::CapacityRecovery
        }
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

fn access_error(error: layer2_core::resource_access::ResourceAccessError) -> StatusCode {
    use layer2_core::resource_access::{
        AccessError, MembershipError, ResourceAccessError::*, ResourceAccessStoreError,
    };
    match error {
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
    }
}

pub(super) fn refusal(status: StatusCode) -> axum::response::Response {
    use axum::response::IntoResponse;
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

fn read_error(error: layer2_core::sandbox::AgentReadError) -> StatusCode {
    use layer2_core::sandbox::AgentReadError;
    match error {
        AgentReadError::NotFound => StatusCode::NOT_FOUND,
        AgentReadError::Forbidden => StatusCode::FORBIDDEN,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod custody_tests;
#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod mutation_tests;
