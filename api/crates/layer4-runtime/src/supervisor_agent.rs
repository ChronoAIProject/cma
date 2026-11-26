use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt, FencedReceipt,
    ResponseFrame,
};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentProcessAction,
};

use super::{AgentProcess, Supervisor, MAX_AGENT_SLOTS};
use crate::bridge::Bridge;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::Operation;
use crate::mutation_store::{finish, Begin, ProcessFilter, Ticket};
use crate::paths::{path_is_directory, safe_component};

#[path = "supervisor_agent_receipts.rs"]
mod receipts;
use receipts::{
    close_response, complete_agent_home, complete_agent_open, home_response, open_response,
    remove_home_response,
};

impl Supervisor {
    pub(super) async fn install_agent_skill(
        &self,
        authority_fence: u64,
        request: layer3_adapter::AgentSkillInstallRequest,
    ) -> Result<ResponseFrame> {
        crate::agent_skill_install::validate(&request)?;
        let operation = Operation::install_agent_skill(&request);
        if !self.mutations.admitted(&operation).await? {
            crate::agent_skill_install::preflight(&self.config, &request)?;
        }
        match self
            .mutations
            .begin::<layer3_adapter::AgentSkillInstallReceipt>(&operation)
            .await?
        {
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
        .await
    }

    pub(super) async fn materialize_agent_home(
        &self,
        authority_fence: u64,
        plan: AgentHomePlan,
    ) -> Result<ResponseFrame> {
        super::agent_validation::materialize_home(&plan)?;
        let operation = Operation::create_agent_home(&plan);
        if !self.mutations.admitted(&operation).await? {
            if self.agents.read().await.contains_key(&plan.agent_id)
                || self
                    .mutations
                    .active(ProcessFilter::Agent(plan.agent_id.as_str().into()))
                    .await?
            {
                return Err(RuntimeError::rejected(
                    "agent_running",
                    "agent process is already open",
                ));
            }
            crate::agent_home::preflight_materialize(&self.config, &plan)?;
        }
        match self.mutations.begin::<AgentHomeReceipt>(&operation).await? {
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
            Ok(Some(path)) => {
                return complete_agent_home(authority_fence, ticket, &plan, path).await
            }
            Ok(None) => {}
            Err(error) if error.retryable => return Err(error),
            Err(error) => {
                tracing::warn!(detail = %error, "agent home creation outcome is indeterminate");
                return Err(ticket.indeterminate(
                    "agent home creation left ambiguous staged or published state; explicit recovery is required",
                ).await?);
            }
        }
        match crate::agent_home::materialize(&self.config, &self.http, &plan).await {
            Ok(path) => complete_agent_home(authority_fence, ticket, &plan, path).await,
            Err(error) => {
                self.finish_failed_agent_home(authority_fence, &plan, ticket, error)
                    .await
            }
        }
    }

    async fn finish_failed_agent_home(
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
            Ok(Some(path)) => complete_agent_home(authority_fence, ticket, plan, path).await,
            Ok(None) if error.retryable => Err(error),
            Ok(None) => {
                ticket.fail(&error).await?;
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
                ).await?)
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
            || self
                .mutations
                .active(ProcessFilter::Agent(request.agent_id.as_str().into()))
                .await?
        {
            return Err(RuntimeError::rejected(
                "agent_running",
                "agent home cannot be removed while its process is logically open",
            ));
        }
        if !self.mutations.admitted(&operation).await? {
            crate::agent_home::preflight_remove(&self.config, request.agent_id.as_str())?;
        }
        match self
            .mutations
            .begin::<AgentHomeRemoveReceipt>(&operation)
            .await?
        {
            Begin::Completed(receipt) => Ok(remove_home_response(authority_fence, receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_remove_agent_home(authority_fence, request, ticket)
                    .await
            }
        }
    }

    async fn execute_remove_agent_home(
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
        .await
    }

    pub(super) async fn open_agent(
        &self,
        authority_fence: u64,
        launch: AgentLaunch,
    ) -> Result<ResponseFrame> {
        super::agent_validation::open(&launch)?;
        let operation = Operation::open_agent(&launch);
        if !self.mutations.admitted(&operation).await? {
            self.validate_launch(&launch).await?;
            self.launch_environment(&launch)?;
        }
        match self.mutations.begin::<AgentOpenReceipt>(&operation).await? {
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
        let mut environment = match self.launch_environment(&launch) {
            Ok(environment) => environment,
            Err(error) => {
                ticket.fail(&error).await?;
                return Err(error);
            }
        };
        if self.credential.has_github_repository_scope().await {
            environment.insert(
                crate::github_proxy::ACCESS_PATH_ENV.into(),
                crate::upstream::github_scope::ACCESS_PATH.into(),
            );
        }

        let bridge = match Bridge::start_with_recovery(
            &launch,
            &environment,
            self.config.capacity_recovery_enabled,
        ) {
            Ok(bridge) => Arc::new(bridge),
            Err(error) => {
                return Err(ticket
                    .indeterminate(&format!(
                        "agent bridge spawn did not produce a durable outcome: {error}"
                    ))
                    .await?)
            }
        };
        self.agents.write().await.insert(
            launch.agent_id.clone(),
            AgentProcess {
                launch: launch.clone(),
                bridge,
            },
        );
        complete_agent_open(authority_fence, ticket, launch).await
    }

    async fn reconcile_started_open(
        &self,
        authority_fence: u64,
        launch: AgentLaunch,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        if self.live_launch_matches(&launch).await {
            complete_agent_open(authority_fence, ticket, launch).await
        } else {
            Err(ticket.indeterminate(
                "agent open may have spawned before its outcome became durable; a duplicate process will not be started",
            ).await?)
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
        if self
            .mutations
            .active(ProcessFilter::Agent(launch.agent_id.as_str().into()))
            .await?
        {
            return Err(RuntimeError::rejected(
                "agent_already_open",
                "agent slot has a durable open without a completed close",
            ));
        }
        if self
            .mutations
            .active(ProcessFilter::Worktree(launch.cwd.clone()))
            .await?
        {
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
        match self
            .mutations
            .begin::<AgentCloseReceipt>(&operation)
            .await?
        {
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
        // Dispatch and journal recovery hold operation_gate through termination,
        // removal, and receipt publication.
        if let Some(bridge) = self.close_process_bridge(&request).await? {
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
        ticket.complete(&receipt).await?;
        Ok(close_response(authority_fence, receipt))
    }

    pub(super) async fn close_process_bridge(
        &self,
        request: &AgentCloseRequest,
    ) -> Result<Option<Arc<Bridge>>> {
        let agent = request.agent_id.as_str();
        let incarnation = super::agent_validation::process_incarnation(
            AgentProcessAction::Close,
            agent,
            &request.operation_id,
        )?;
        let agents = self.agents.read().await;
        let Some(process) = agents.get(&request.agent_id) else {
            return Ok(None);
        };
        let opened = AgentProcessAction::Open.incarnation(agent, &process.launch.operation_id);
        if opened != Some(incarnation) {
            return Err(RuntimeError::rejected(
                "agent_process_incarnation_changed",
                "the close request belongs to a different Agent process",
            ));
        }
        Ok(Some(Arc::clone(&process.bridge)))
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
