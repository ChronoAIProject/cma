use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt, FencedReceipt,
    ResponseFrame,
};
use layer3_adapter::{AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch};

use super::{AgentProcess, Supervisor, MAX_AGENT_SLOTS};
use crate::bridge::Bridge;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{self, Begin, Operation, Ticket};
use crate::paths::{path_is_directory, safe_component};

impl Supervisor {
    pub(super) async fn install_agent_skill(
        &self,
        authority_fence: u64,
        request: layer3_adapter::AgentSkillInstallRequest,
    ) -> Result<ResponseFrame> {
        crate::agent_skill_install::validate(&request)?;
        let operation = Operation::install_agent_skill(&request);
        if !mutation_journal::admitted(&self.config, &operation)? {
            crate::agent_skill_install::preflight(&self.config, &request)?;
        }
        match mutation_journal::begin::<layer3_adapter::AgentSkillInstallReceipt>(
            &self.config,
            &operation,
        )? {
            Begin::Completed(receipt) => Ok(ResponseFrame::InstallAgentSkill(FencedReceipt::new(
                authority_fence,
                receipt,
            ))),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_install_agent_skill(authority_fence, request, ticket)
                    .await
            }
        }
    }
    pub(super) async fn execute_install_agent_skill(
        &self,
        authority_fence: u64,
        request: layer3_adapter::AgentSkillInstallRequest,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        let result = crate::agent_skill_install::install(&self.config, &self.http, &request).await;
        finish(ticket, result, |receipt| {
            ResponseFrame::InstallAgentSkill(FencedReceipt::new(authority_fence, receipt))
        })
    }

    pub(super) async fn materialize_agent_home(
        &self,
        authority_fence: u64,
        plan: AgentHomePlan,
    ) -> Result<ResponseFrame> {
        super::agent_validation::materialize_home(&plan)?;
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
            Begin::Completed(receipt) => Ok(home_response(authority_fence, receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_materialize_agent_home(authority_fence, plan, ticket)
                    .await
            }
        }
    }

    async fn execute_materialize_agent_home(
        &self,
        authority_fence: u64,
        plan: AgentHomePlan,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        match crate::agent_home::recover_materialization(
            &self.config,
            plan.agent_id.as_str(),
            &plan.operation_id,
        ) {
            Ok(Some(path)) => return complete_agent_home(authority_fence, ticket, &plan, path),
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
            Ok(path) => complete_agent_home(authority_fence, ticket, &plan, path),
            Err(error) => self.finish_failed_agent_home(authority_fence, &plan, ticket, error),
        }
    }

    fn finish_failed_agent_home(
        &self,
        authority_fence: u64,
        plan: &AgentHomePlan,
        ticket: Ticket,
        error: RuntimeError,
    ) -> Result<ResponseFrame> {
        match crate::agent_home::recover_materialization(
            &self.config,
            plan.agent_id.as_str(),
            &plan.operation_id,
        ) {
            Ok(Some(path)) => complete_agent_home(authority_fence, ticket, plan, path),
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
        authority_fence: u64,
        request: AgentHomeRemoveRequest,
    ) -> Result<ResponseFrame> {
        super::agent_validation::remove_home(&request)?;
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
            Begin::Completed(receipt) => Ok(remove_home_response(authority_fence, receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_remove_agent_home(authority_fence, request, ticket)
            }
        }
    }

    fn execute_remove_agent_home(
        &self,
        authority_fence: u64,
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
        finish(ticket, result, |receipt| {
            remove_home_response(authority_fence, receipt)
        })
    }

    pub(super) async fn open_agent(
        &self,
        authority_fence: u64,
        launch: AgentLaunch,
    ) -> Result<ResponseFrame> {
        super::agent_validation::open(&launch)?;
        let operation = Operation::open_agent(&launch);
        if !mutation_journal::admitted(&self.config, &operation)? {
            self.validate_launch(&launch).await?;
            self.launch_environment(&launch)?;
        }
        match mutation_journal::begin::<AgentOpenReceipt>(&self.config, &operation)? {
            Begin::Completed(receipt) => {
                if self.live_launch_matches(&launch).await {
                    Ok(open_response(authority_fence, receipt))
                } else {
                    Err(RuntimeError::rejected(
                        "agent_process_not_live",
                        "completed agent open cannot replay without its matching live bridge",
                    ))
                }
            }
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) => {
                self.execute_open_agent(authority_fence, launch, ticket)
                    .await
            }
            Begin::Started(ticket) => {
                self.reconcile_started_open(authority_fence, launch, ticket)
                    .await
            }
        }
    }

    async fn execute_open_agent(
        &self,
        authority_fence: u64,
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
        complete_agent_open(authority_fence, ticket, launch)
    }

    async fn reconcile_started_open(
        &self,
        authority_fence: u64,
        launch: AgentLaunch,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        if self.live_launch_matches(&launch).await {
            complete_agent_open(authority_fence, ticket, launch)
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

    pub(super) async fn close_agent(
        &self,
        authority_fence: u64,
        request: AgentCloseRequest,
    ) -> Result<ResponseFrame> {
        super::agent_validation::close(&request)?;
        safe_component(request.agent_id.as_str(), "agent id")?;
        let operation = Operation::close_agent(&request);
        match mutation_journal::begin::<AgentCloseReceipt>(&self.config, &operation)? {
            Begin::Completed(receipt) => Ok(close_response(authority_fence, receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_close_agent(authority_fence, request, ticket)
                    .await
            }
        }
    }

    pub(super) async fn execute_close_agent(
        &self,
        authority_fence: u64,
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
            self.browsers.lock().await.remove(&request.agent_id);
        }
        let receipt = AgentCloseReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: request.agent_id,
        };
        ticket.complete(&receipt)?;
        Ok(close_response(authority_fence, receipt))
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
            if (super::reserved(name)
                || (self.config.github_proxy.is_some()
                    && crate::github_proxy::reserved_environment(name)))
                && environment.get(name) != Some(value)
            {
                return Err(RuntimeError::rejected(
                    "agent_environment_conflict",
                    format!("agent environment cannot override reserved variable {name}"),
                ));
            }
            environment.insert(name.clone(), value.clone());
        }
        environment.insert("HOME".into(), launch.home.clone());
        environment.insert(
            "CMA_BROWSER_SOCKET".into(),
            crate::browser::directory(&self.config, &self.boot_id, launch.agent_id.as_str())
                .join("browser.sock")
                .to_string_lossy()
                .into_owned(),
        );
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
    authority_fence: u64,
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
    Ok(home_response(authority_fence, receipt))
}

fn complete_agent_open(
    authority_fence: u64,
    ticket: Ticket,
    launch: AgentLaunch,
) -> Result<ResponseFrame> {
    let receipt = AgentOpenReceipt {
        operation_id: launch.operation_id,
        request_sha256: launch.request_sha256,
        agent_id: launch.agent_id,
        cwd: launch.cwd,
        home: launch.home,
        conversation_id: None,
    };
    ticket.complete(&receipt)?;
    Ok(open_response(authority_fence, receipt))
}

fn home_response(authority_fence: u64, receipt: AgentHomeReceipt) -> ResponseFrame {
    ResponseFrame::MaterializeAgentHome(FencedReceipt::new(authority_fence, receipt))
}

fn remove_home_response(authority_fence: u64, receipt: AgentHomeRemoveReceipt) -> ResponseFrame {
    ResponseFrame::RemoveAgentHome(FencedReceipt::new(authority_fence, receipt))
}

fn open_response(authority_fence: u64, receipt: AgentOpenReceipt) -> ResponseFrame {
    ResponseFrame::OpenAgent(FencedReceipt::new(authority_fence, receipt))
}

fn close_response(authority_fence: u64, receipt: AgentCloseReceipt) -> ResponseFrame {
    ResponseFrame::CloseAgent(FencedReceipt::new(authority_fence, receipt))
}
