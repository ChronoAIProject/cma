use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentProcessAction,
};

use crate::failure::{Result, RuntimeError};

pub(super) fn materialize_home(plan: &AgentHomePlan) -> Result<()> {
    validate_forwarded_identity(
        &plan.operation_id,
        &plan.request_sha256,
        plan.operation_id == format!("agent:{}:home:create:v1", plan.agent_id.as_str()),
    )
}

pub(super) fn remove_home(request: &AgentHomeRemoveRequest) -> Result<()> {
    validate_agent_request(
        request.has_valid_sha256(),
        &request.operation_id,
        &request.request_sha256,
        request.operation_id == format!("agent:{}:home:remove:v1", request.agent_id.as_str()),
    )
}

pub(super) fn open(launch: &AgentLaunch) -> Result<u64> {
    validate_forwarded_identity(&launch.operation_id, &launch.request_sha256, true)?;
    process_incarnation(
        AgentProcessAction::Open,
        launch.agent_id.as_str(),
        &launch.operation_id,
    )
}

pub(super) fn close(request: &AgentCloseRequest) -> Result<u64> {
    validate_agent_request(
        request.has_valid_sha256(),
        &request.operation_id,
        &request.request_sha256,
        true,
    )?;
    process_incarnation(
        AgentProcessAction::Close,
        request.agent_id.as_str(),
        &request.operation_id,
    )
}

pub(super) fn process_incarnation(
    action: AgentProcessAction,
    agent: &str,
    operation_id: &str,
) -> Result<u64> {
    action.incarnation(agent, operation_id).ok_or_else(|| {
        RuntimeError::rejected(
            "agent_operation_invalid",
            "agent operation identity is invalid",
        )
    })
}

fn validate_agent_request(
    digest_matches: bool,
    operation_id: &str,
    request_sha256: &str,
    identity_matches: bool,
) -> Result<()> {
    if !digest_matches {
        return Err(RuntimeError::rejected(
            "agent_request_digest_invalid",
            "agent request digest does not match its execution fields",
        ));
    }
    validate_forwarded_identity(operation_id, request_sha256, identity_matches)
}

fn validate_forwarded_identity(
    operation_id: &str,
    request_sha256: &str,
    identity_matches: bool,
) -> Result<()> {
    let lower_sha256 = request_sha256.len() == 64
        && request_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !identity_matches || operation_id.len() > 256 || !lower_sha256 {
        return Err(RuntimeError::rejected(
            "agent_operation_invalid",
            "agent operation identity is invalid",
        ));
    }
    Ok(())
}
