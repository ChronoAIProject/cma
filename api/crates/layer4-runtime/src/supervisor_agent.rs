use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt, ResponseFrame,
};
use layer3_adapter::{AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch};

use super::{AgentProcess, Supervisor, MAX_AGENT_SLOTS};
use crate::bridge::Bridge;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{self, Begin, Operation, Ticket};
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
        let operation = Operation::create_agent_home(&plan);
        if !mutation_journal::admitted(&self.config, &operation)? {
            if self.agents.read().await.contains_key(&plan.agent_id)
                || mutation_journal::active_agents(&self.config)?
                    .iter()
                    .any(|agent| agent.agent_id == plan.agent_id.as_str())
            {
                return Err(RuntimeError::rejected(
                    "agent_running",
                    "agent process is already open",
                ));
            }
            crate::agent_home::preflight_materialize(&self.config, &plan)?;
        }
        match mutation_journal::begin::<AgentHomeReceipt>(&self.config, &operation)? {
            Begin::Completed(receipt) => Ok(ResponseFrame::MaterializeAgentHome(receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_materialize_agent_home(plan, ticket).await
            }
        }
    }

    async fn execute_materialize_agent_home(
        &self,
        plan: AgentHomePlan,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        match crate::agent_home::recover_materialization(
            &self.config,
            plan.agent_id.as_str(),
            &plan.operation_id,
        ) {
            Ok(Some(path)) => return complete_agent_home(ticket, &plan, path),
            Ok(None) => {}
            Err(error) if error.retryable => return Err(error),
            Err(error) => {
                tracing::warn!(detail = %error, "agent home creation outcome is indeterminate");
                return Err(ticket.indeterminate(
                    "agent home creation left ambiguous staged or published state; explicit recovery is required",
                )?);
            }
        }
        match crate::agent_home::materialize(&self.config, &self.http, &plan).await {
            Ok(path) => complete_agent_home(ticket, &plan, path),
            Err(error) => self.finish_failed_agent_home(&plan, ticket, error),
        }
    }

    fn finish_failed_agent_home(
        &self,
        plan: &AgentHomePlan,
        ticket: Ticket,
        error: RuntimeError,
    ) -> Result<ResponseFrame> {
        match crate::agent_home::recover_materialization(
            &self.config,
            plan.agent_id.as_str(),
            &plan.operation_id,
        ) {
            Ok(Some(path)) => complete_agent_home(ticket, plan, path),
            Ok(None) if error.retryable => Err(error),
            Ok(None) => {
                ticket.fail(&error)?;
                Err(error)
            }
            Err(observation) if observation.retryable => Err(observation),
            Err(observation) => {
                tracing::warn!(
                    original = %error,
                    detail = %observation,
                    "agent home creation outcome is indeterminate"
                );
                Err(ticket.indeterminate(
                    "agent home creation left ambiguous staged or published state; explicit recovery is required",
                )?)
            }
        }
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
        let operation = Operation::remove_agent_home(&request);
        if self.agents.read().await.contains_key(&request.agent_id)
            || mutation_journal::active_agents(&self.config)?
                .iter()
                .any(|agent| agent.agent_id == request.agent_id.as_str())
        {
            return Err(RuntimeError::rejected(
                "agent_running",
                "agent home cannot be removed while its process is logically open",
            ));
        }
        if !mutation_journal::admitted(&self.config, &operation)? {
            crate::agent_home::preflight_remove(&self.config, request.agent_id.as_str())?;
        }
        match mutation_journal::begin::<AgentHomeRemoveReceipt>(&self.config, &operation)? {
            Begin::Completed(receipt) => Ok(ResponseFrame::RemoveAgentHome(receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_remove_agent_home(request, ticket)
            }
        }
    }

    fn execute_remove_agent_home(
        &self,
        request: AgentHomeRemoveRequest,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        let result = crate::agent_home::remove(&self.config, request.agent_id.as_str()).map(|_| {
            AgentHomeRemoveReceipt {
                operation_id: request.operation_id,
                request_sha256: request.request_sha256,
                agent_id: request.agent_id,
            }
        });
        finish(ticket, result, ResponseFrame::RemoveAgentHome)
    }

    pub(super) async fn open_agent(&self, launch: AgentLaunch) -> Result<ResponseFrame> {
        validate_forwarded_identity(
            &launch.operation_id,
            &launch.request_sha256,
            &format!("agent:{}:open:v1", launch.agent_id.as_str()),
        )?;
        let operation = Operation::open_agent(&launch);
        if !mutation_journal::admitted(&self.config, &operation)? {
            self.validate_launch(&launch).await?;
            self.launch_environment(&launch)?;
        }
        match mutation_journal::begin::<AgentOpenReceipt>(&self.config, &operation)? {
            Begin::Completed(receipt) => {
                if self.live_launch_matches(&launch).await {
                    Ok(ResponseFrame::OpenAgent(receipt))
                } else {
                    Err(RuntimeError::rejected(
                        "agent_process_not_live",
                        "completed agent open cannot replay without its matching live bridge",
                    ))
                }
            }
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) => self.execute_open_agent(launch, ticket).await,
            Begin::Started(ticket) => self.reconcile_started_open(launch, ticket).await,
        }
    }

    async fn execute_open_agent(
        &self,
        launch: AgentLaunch,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        let environment = match self.launch_environment(&launch) {
            Ok(environment) => environment,
            Err(error) => {
                ticket.fail(&error)?;
                return Err(error);
            }
        };
        let bridge = match Bridge::start(&launch, &environment) {
            Ok(bridge) => Arc::new(bridge),
            Err(error) => {
                return Err(ticket.indeterminate(&format!(
                    "agent bridge spawn did not produce a durable outcome: {error}"
                ))?)
            }
        };
        self.agents.write().await.insert(
            launch.agent_id.clone(),
            AgentProcess {
                launch: launch.clone(),
                bridge,
            },
        );
        complete_agent_open(ticket, launch)
    }

    async fn reconcile_started_open(
        &self,
        launch: AgentLaunch,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        if self.live_launch_matches(&launch).await {
            complete_agent_open(ticket, launch)
        } else {
            Err(ticket.indeterminate(
                "agent open may have spawned before its outcome became durable; a duplicate process will not be started",
            )?)
        }
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
        let durable = mutation_journal::active_agents(&self.config)?;
        if durable
            .iter()
            .any(|agent| agent.agent_id == launch.agent_id.as_str())
        {
            return Err(RuntimeError::rejected(
                "agent_already_open",
                "agent slot has a durable open without a completed close",
            ));
        }
        if durable.iter().any(|agent| agent.cwd == launch.cwd) {
            return Err(RuntimeError::rejected(
                "worktree_agent_exists",
                "worktree already has a durable agent open",
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
        let operation = Operation::close_agent(&request);
        match mutation_journal::begin::<AgentCloseReceipt>(&self.config, &operation)? {
            Begin::Completed(receipt) => Ok(ResponseFrame::CloseAgent(receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_close_agent(request, ticket).await
            }
        }
    }

    pub(super) async fn execute_close_agent(
        &self,
        request: AgentCloseRequest,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        let bridge = self
            .agents
            .read()
            .await
            .get(&request.agent_id)
            .map(|process| Arc::clone(&process.bridge));
        if let Some(bridge) = bridge {
            bridge
                .terminate()
                .await
                .map_err(|error| RuntimeError::unavailable("agent_termination_failed", error))?;
            self.agents.write().await.remove(&request.agent_id);
        }
        let receipt = AgentCloseReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: request.agent_id,
        };
        ticket.complete(&receipt)?;
        Ok(ResponseFrame::CloseAgent(receipt))
    }

    async fn live_launch_matches(&self, launch: &AgentLaunch) -> bool {
        self.agents
            .read()
            .await
            .get(&launch.agent_id)
            .is_some_and(|process| process.launch == *launch && !process.bridge.is_ended())
    }

    fn launch_environment(&self, launch: &AgentLaunch) -> Result<BTreeMap<String, String>> {
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
        Ok(environment)
    }
}

fn finish<T: serde::Serialize>(
    ticket: Ticket,
    result: Result<T>,
    response: impl FnOnce(T) -> ResponseFrame,
) -> Result<ResponseFrame> {
    match result {
        Ok(receipt) => {
            ticket.complete(&receipt)?;
            Ok(response(receipt))
        }
        Err(error) => {
            if !error.retryable {
                ticket.fail(&error)?;
            }
            Err(error)
        }
    }
}

fn complete_agent_home(
    ticket: Ticket,
    plan: &AgentHomePlan,
    path: std::path::PathBuf,
) -> Result<ResponseFrame> {
    let receipt = AgentHomeReceipt {
        operation_id: plan.operation_id.clone(),
        request_sha256: plan.request_sha256.clone(),
        agent_id: plan.agent_id.clone(),
        path: path.to_string_lossy().into_owned(),
    };
    ticket.complete(&receipt)?;
    Ok(ResponseFrame::MaterializeAgentHome(receipt))
}

fn complete_agent_open(ticket: Ticket, launch: AgentLaunch) -> Result<ResponseFrame> {
    let receipt = AgentOpenReceipt {
        operation_id: launch.operation_id,
        request_sha256: launch.request_sha256,
        agent_id: launch.agent_id,
        cwd: launch.cwd,
        home: launch.home,
        conversation_id: None,
    };
    ticket.complete(&receipt)?;
    Ok(ResponseFrame::OpenAgent(receipt))
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
