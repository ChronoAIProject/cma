use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::profiles::store::ProfileStoreError;
use crate::{Caller, Capability, ServiceId};

use super::{
    CreateOrResumeLaunch, Launch, LaunchAdmissionResult, LaunchCreateResult, LaunchEventKind,
    LaunchServiceError, LaunchStoreError, PublicTriggerId, RequirementsSnapshot, ResolvedResource,
    ServiceConnectionError, StartLaunch, TriggerStoreError,
};

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchPolicy {
    pub enabled: bool,
    pub orchestration_paused: bool,
    pub visitor_launches_per_day: u32,
}

#[async_trait]
pub trait LaunchPolicySource: Send + Sync {
    async fn current_launch_policy(&self) -> Result<LaunchPolicy, LaunchServiceError>;
}

#[async_trait]
impl LaunchPolicySource for LaunchPolicy {
    async fn current_launch_policy(&self) -> Result<LaunchPolicy, LaunchServiceError> {
        Ok(*self)
    }
}

pub(super) fn require_launcher(caller: &Caller) -> Result<(), LaunchServiceError> {
    if caller.authorization.allows(Capability::LaunchTriggers)
        || caller.authorization.allows(Capability::CreateSandbox)
    {
        Ok(())
    } else {
        Err(LaunchServiceError::Forbidden)
    }
}

pub(super) fn validate_create_request(
    request: &CreateOrResumeLaunch,
) -> Result<(), LaunchServiceError> {
    let key = request.idempotency_key.as_bytes();
    if key.is_empty()
        || key.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || !key.iter().all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(LaunchServiceError::InvalidIdempotencyKey);
    }
    if let Some(origin) = request.referrer_origin.as_deref() {
        validate_referrer_origin(origin)?;
    }
    Ok(())
}

pub(super) fn validate_start_request(request: &StartLaunch) -> Result<(), LaunchServiceError> {
    if request
        .trust_confirmed_digest
        .as_deref()
        .is_some_and(|digest| !valid_digest(digest))
        || request.acknowledgements.len() > 32
        || request
            .acknowledgements
            .iter()
            .any(|value| value.trim().is_empty() || value.len() > 128)
    {
        return Err(LaunchServiceError::InvalidRequest(
            "launch confirmation is invalid".into(),
        ));
    }
    Ok(())
}

pub(super) fn command_hash(key: &str) -> String {
    separated_hash("cma.trigger-launch.command-key.v1", key.as_bytes())
}

pub(super) fn request_fingerprint(
    public_id: &PublicTriggerId,
    request: &CreateOrResumeLaunch,
) -> Result<String, LaunchServiceError> {
    let bytes = serde_json::to_vec(&LaunchRequestIdentity {
        public_id,
        placement_id: request.placement_id.as_ref(),
        referrer_origin: request.referrer_origin.as_deref(),
        start_if_ready: request.start_if_ready,
        choice: request.choice,
    })
    .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
    Ok(separated_hash(
        "cma.trigger-launch.request-fingerprint.v1",
        &bytes,
    ))
}

pub(super) fn validate_requirement_identity(
    expected: &[ServiceId],
    snapshot: &RequirementsSnapshot,
) -> Result<(), LaunchServiceError> {
    let actual = snapshot
        .services
        .iter()
        .map(|requirement| &requirement.service)
        .collect::<Vec<_>>();
    if expected.iter().eq(actual) {
        Ok(())
    } else {
        Err(LaunchServiceError::Corrupt(
            "service requirements do not match the frozen Sandbox Profile".into(),
        ))
    }
}

pub(super) fn append_resolution_events(launch: &Launch, events: &mut Vec<LaunchEventKind>) {
    match &launch.resolution.sandbox {
        ResolvedResource::Reused(id) => events.push(LaunchEventKind::SandboxResolved {
            sandbox_id: id.clone(),
            reused: true,
        }),
        ResolvedResource::Created(id) => events.push(LaunchEventKind::SandboxResolved {
            sandbox_id: id.clone(),
            reused: false,
        }),
        ResolvedResource::Pending => {}
    }
    match &launch.resolution.workspace {
        ResolvedResource::Reused(id) => events.push(LaunchEventKind::WorkspaceResolved {
            workspace_id: id.clone(),
            reused: true,
        }),
        ResolvedResource::Created(id) => events.push(LaunchEventKind::WorkspaceResolved {
            workspace_id: id.clone(),
            reused: false,
        }),
        ResolvedResource::Pending => {}
    }
    match &launch.resolution.agent {
        ResolvedResource::Reused(id) => events.push(LaunchEventKind::AgentResolved {
            agent_id: id.clone(),
            reused: true,
        }),
        ResolvedResource::Created(id) => events.push(LaunchEventKind::AgentResolved {
            agent_id: id.clone(),
            reused: false,
        }),
        ResolvedResource::Pending => {}
    }
}

pub(super) fn create_result(result: LaunchAdmissionResult) -> LaunchCreateResult {
    let created = result.was_created();
    LaunchCreateResult {
        launch: match result {
            LaunchAdmissionResult::Created(launch) | LaunchAdmissionResult::Existing(launch) => {
                launch
            }
        },
        created,
    }
}

pub(super) fn profile_error(error: ProfileStoreError) -> LaunchServiceError {
    match error {
        ProfileStoreError::Unavailable(detail) => LaunchServiceError::Unavailable(detail),
        ProfileStoreError::Corrupt(detail) => LaunchServiceError::Corrupt(detail),
        other => LaunchServiceError::Corrupt(other.to_string()),
    }
}

pub(super) fn trigger_error(error: TriggerStoreError) -> LaunchServiceError {
    match error {
        TriggerStoreError::Unavailable(detail) => LaunchServiceError::Unavailable(detail),
        TriggerStoreError::Corrupt(detail) => LaunchServiceError::Corrupt(detail),
        TriggerStoreError::NotFound => LaunchServiceError::TriggerNotFound,
        other => LaunchServiceError::Corrupt(other.to_string()),
    }
}

pub(super) fn connection_error(error: ServiceConnectionError) -> LaunchServiceError {
    match error {
        ServiceConnectionError::Malformed(detail) => LaunchServiceError::Corrupt(detail),
        other => LaunchServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn store_error(error: LaunchStoreError) -> LaunchServiceError {
    match error {
        LaunchStoreError::VisitorQuota { limit, reset_at } => {
            LaunchServiceError::VisitorQuota { limit, reset_at }
        }
        LaunchStoreError::NotFound => LaunchServiceError::NotFound,
        LaunchStoreError::CommandConflict => LaunchServiceError::IdempotencyKeyReused,
        LaunchStoreError::Conflict { .. } | LaunchStoreError::LeaseHeld => {
            LaunchServiceError::Conflict
        }
        LaunchStoreError::InvalidCursor => LaunchServiceError::InvalidCursor,
        LaunchStoreError::TriggerUnavailable => LaunchServiceError::TriggerNotFound,
        LaunchStoreError::TriggerRetired => LaunchServiceError::TriggerRetired,
        LaunchStoreError::Unavailable(detail) => LaunchServiceError::Unavailable(detail),
        LaunchStoreError::Corrupt(detail) => LaunchServiceError::Corrupt(detail),
        LaunchStoreError::Budget(reason) => LaunchServiceError::Budget(reason),
        LaunchStoreError::AdmissionPaused => LaunchServiceError::AdmissionPaused,
        other => LaunchServiceError::Corrupt(other.to_string()),
    }
}

#[derive(Serialize)]
struct LaunchRequestIdentity<'a> {
    public_id: &'a PublicTriggerId,
    placement_id: Option<&'a super::PlacementId>,
    referrer_origin: Option<&'a str>,
    start_if_ready: bool,
    choice: Option<super::RepeatChoice>,
}

fn separated_hash(domain: &str, value: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(value);
    format!("{:x}", digest.finalize())
}

fn validate_referrer_origin(value: &str) -> Result<(), LaunchServiceError> {
    let url = url::Url::parse(value).map_err(|_| invalid_referrer())?;
    if value.len() > 512
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(invalid_referrer());
    }
    Ok(())
}

fn invalid_referrer() -> LaunchServiceError {
    LaunchServiceError::InvalidRequest("referrer_origin is invalid".into())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
