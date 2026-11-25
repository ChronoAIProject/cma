use super::super::super::reply::valid_sha256;
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentOpenRequest,
    AgentProcessAction, WorktreeRef, WorktreeRequest,
};

pub(super) trait ReplayRequest {
    fn operation_id(&self) -> &str;
    fn request_sha256(&self) -> &str;
    fn digest_matches(&self) -> bool;
}

macro_rules! replay_request {
    ($($type:ty),+ $(,)?) => {
        $(impl ReplayRequest for $type {
            fn operation_id(&self) -> &str { &self.operation_id }
            fn request_sha256(&self) -> &str { &self.request_sha256 }
            fn digest_matches(&self) -> bool { self.has_valid_sha256() }
        })+
    };
}

replay_request!(
    WorktreeRequest,
    WorktreeRef,
    AgentHomeRequest,
    AgentHomeRemoveRequest,
    AgentOpenRequest,
    AgentCloseRequest,
);

pub(super) fn validate_request(
    request: &impl ReplayRequest,
    expected: String,
) -> crate::Result<()> {
    validate_request_identity(request, request.operation_id() == expected)
}

pub(super) fn validate_process_request(
    request: &impl ReplayRequest,
    action: AgentProcessAction,
    agent: &str,
) -> crate::Result<()> {
    validate_request_identity(
        request,
        action.incarnation(agent, request.operation_id()).is_some(),
    )
}

fn validate_request_identity(request: &impl ReplayRequest, matches: bool) -> crate::Result<()> {
    if !matches
        || request.operation_id().len() > 256
        || !valid_sha256(request.request_sha256())
        || !request.digest_matches()
    {
        return Err(AdapterError::Rejected(
            "operation id or request digest is invalid".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_fence(authority_fence: u64) -> crate::Result<()> {
    if authority_fence == 0 {
        Err(AdapterError::Rejected(
            "agent mutation authority fence must be nonzero".into(),
        ))
    } else {
        Ok(())
    }
}
