use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt, ResponseFrame,
};
use layer3_adapter::{AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch};

use super::{AgentProcess, Supervisor, MAX_AGENT_SLOTS};
use crate::bridge::Bridge;
use crate::failure::{Result, RuntimeError};
use crate::paths::{path_is_directory, safe_component};

impl Supervisor {
    pub(super) async fn materialize_agent_home(
        &self,
        plan: AgentHomePlan,
    ) -> Result<ResponseFrame> {
        validate_forwarded_identity(
            &plan.operation_id,
            &plan.request_sha256,
            &format!("agent:{}:home:create:v1", plan.agent_id.as_str()),
        )?;
        if self.agents.read().await.contains_key(&plan.agent_id) {
            return Err(RuntimeError::rejected(
                "agent_running",
                "agent process is already open",
            ));
        }
        let path = crate::agent_home::materialize(&self.config, &self.http, &plan).await?;
        Ok(ResponseFrame::MaterializeAgentHome(AgentHomeReceipt {
            operation_id: plan.operation_id,
            request_sha256: plan.request_sha256,
            agent_id: plan.agent_id,
            path: path.to_string_lossy().into_owned(),
        }))
    }

    pub(super) async fn remove_agent_home(
        &self,
        request: AgentHomeRemoveRequest,
    ) -> Result<ResponseFrame> {
        validate_agent_request(
            request.has_valid_sha256(),
            &request.operation_id,
            &request.request_sha256,
            &format!("agent:{}:home:remove:v1", request.agent_id.as_str()),
        )?;
        if self.agents.read().await.contains_key(&request.agent_id) {
            return Err(RuntimeError::rejected(
                "agent_running",
                "agent home cannot be removed while its process is open",
            ));
        }
        if !crate::agent_home::remove(&self.config, request.agent_id.as_str())? {
            return Err(RuntimeError::rejected(
                "agent_home_not_found",
                "agent home was not found",
            ));
        }
        Ok(ResponseFrame::RemoveAgentHome(AgentHomeRemoveReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: request.agent_id,
        }))
    }

    pub(super) async fn open_agent(&self, launch: AgentLaunch) -> Result<ResponseFrame> {
        validate_forwarded_identity(
            &launch.operation_id,
            &launch.request_sha256,
            &format!("agent:{}:open:v1", launch.agent_id.as_str()),
        )?;
        self.validate_launch(&launch).await?;
        let mut environment = self.config.process_env.clone();
        for (name, value) in &launch.env {
            super::validate_environment(name, value)?;
            if super::reserved(name) && environment.get(name) != Some(value) {
                return Err(RuntimeError::rejected(
                    "agent_environment_conflict",
                    format!("agent environment cannot override reserved variable {name}"),
                ));
            }
            environment.insert(name.clone(), value.clone());
        }
        environment.insert("HOME".into(), launch.home.clone());
        let bridge = Arc::new(Bridge::start(&launch, &environment).map_err(|error| {
            RuntimeError::unavailable("agent_spawn_failed", format!("start agent bridge: {error}"))
        })?);
        self.agents.write().await.insert(
            launch.agent_id.clone(),
            AgentProcess {
                launch: launch.clone(),
                bridge,
            },
        );
        Ok(ResponseFrame::OpenAgent(AgentOpenReceipt {
            operation_id: launch.operation_id,
            request_sha256: launch.request_sha256,
            agent_id: launch.agent_id,
            cwd: launch.cwd,
            home: launch.home,
            conversation_id: None,
        }))
    }

    async fn validate_launch(&self, launch: &AgentLaunch) -> Result<()> {
        safe_component(launch.agent_id.as_str(), "agent id")?;
        let agents = self.agents.read().await;
        if agents.contains_key(&launch.agent_id) {
            return Err(RuntimeError::rejected(
                "agent_already_open",
                "agent slot already has a process",
            ));
        }
        if agents.len() >= MAX_AGENT_SLOTS {
            return Err(RuntimeError::rejected(
                "agent_capacity_reached",
                format!("runtime already has its maximum {MAX_AGENT_SLOTS} agent slots"),
            ));
        }
        drop(agents);
        let expected_home = self.config.agent_home(launch.agent_id.as_str());
        if Path::new(&launch.home) != expected_home || !path_is_directory(&expected_home) {
            return Err(RuntimeError::rejected(
                "agent_home_invalid",
                "agent launch does not name its materialized home",
            ));
        }
        let cwd = Path::new(&launch.cwd);
        if crate::worktree::parse_path(&self.config.home, cwd).is_none()
            || !path_is_directory(cwd)
            || !cwd.join(".git").is_file()
        {
            return Err(RuntimeError::rejected(
                "agent_worktree_invalid",
                "agent launch cwd is not a prepared worktree",
            ));
        }
        if self
            .agents
            .read()
            .await
            .values()
            .any(|agent| agent.launch.cwd == launch.cwd)
        {
            return Err(RuntimeError::rejected(
                "worktree_agent_exists",
                "worktree already has an agent process",
            ));
        }
        if launch.program.is_empty()
            || launch.program.contains('\0')
            || launch.args.iter().any(|argument| argument.contains('\0'))
        {
            return Err(RuntimeError::rejected(
                "agent_program_invalid",
                "agent program or argument contains an invalid value",
            ));
        }
        Ok(())
    }

    pub(super) async fn close_agent(&self, request: AgentCloseRequest) -> Result<ResponseFrame> {
        validate_agent_request(
            request.has_valid_sha256(),
            &request.operation_id,
            &request.request_sha256,
            &format!("agent:{}:close:v1", request.agent_id.as_str()),
        )?;
        safe_component(request.agent_id.as_str(), "agent id")?;
        let bridge = self
            .agents
            .read()
            .await
            .get(&request.agent_id)
            .map(|process| Arc::clone(&process.bridge))
            .ok_or_else(|| {
                RuntimeError::rejected("agent_not_found", "agent process was not found")
            })?;
        bridge
            .terminate()
            .await
            .map_err(|error| RuntimeError::unavailable("agent_termination_failed", error))?;
        self.agents.write().await.remove(&request.agent_id);
        Ok(ResponseFrame::CloseAgent(AgentCloseReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: request.agent_id,
        }))
    }
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
