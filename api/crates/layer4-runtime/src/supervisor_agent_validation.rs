use layer3_adapter::{AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch};

use crate::failure::{Result, RuntimeError};

pub(super) fn materialize_home(plan: &AgentHomePlan) -> Result<()> {
    validate_forwarded_identity(
        &plan.operation_id,
        &plan.request_sha256,
        &format!("agent:{}:home:create:v1", plan.agent_id.as_str()),
    )
}

pub(super) fn remove_home(request: &AgentHomeRemoveRequest) -> Result<()> {
    validate_agent_request(
        request.has_valid_sha256(),
        &request.operation_id,
        &request.request_sha256,
        &format!("agent:{}:home:remove:v1", request.agent_id.as_str()),
    )
}

pub(super) fn open(launch: &AgentLaunch) -> Result<()> {
    validate_forwarded_identity(
        &launch.operation_id,
        &launch.request_sha256,
        &format!("agent:{}:open:v1", launch.agent_id.as_str()),
    )
}

pub(super) fn close(request: &AgentCloseRequest) -> Result<()> {
    validate_agent_request(
        request.has_valid_sha256(),
        &request.operation_id,
        &request.request_sha256,
        &format!("agent:{}:close:v1", request.agent_id.as_str()),
    )
}

fn validate_agent_request(
    digest_matches: bool,
    operation_id: &str,
    request_sha256: &str,
    expected: &str,
) -> Result<()> {
    if !digest_matches {
        return Err(RuntimeError::rejected(
            "agent_request_digest_invalid",
            "agent request digest does not match its execution fields",
        ));
    }
    validate_forwarded_identity(operation_id, request_sha256, expected)
}

fn validate_forwarded_identity(
    operation_id: &str,
    request_sha256: &str,
    expected: &str,
) -> Result<()> {
    let lower_sha256 = request_sha256.len() == 64
        && request_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if operation_id != expected || operation_id.len() > 256 || !lower_sha256 {
        return Err(RuntimeError::rejected(
            "agent_operation_invalid",
            "agent operation identity is invalid",
        ));
    }
    Ok(())
}
