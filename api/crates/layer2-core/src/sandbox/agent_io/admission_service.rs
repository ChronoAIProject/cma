use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::ids::{AgentCommandId, AgentResponseId};
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceCapability, ResourceTarget,
};
use crate::sandbox::{store::SandboxMutationGuard, AgentOperationError};
use crate::{rfc3339_at, AgentId};

use super::*;

pub(super) const IO_BOUND: Duration = Duration::from_secs(15);
const QUEUE_LIFETIME: Duration = Duration::from_secs(300);
const CREATION_ADMISSION_ATTEMPTS: usize = 3;

/// Selected by a trusted interface adapter, separately from client intent.
#[derive(Debug, Clone, Copy)]
pub enum CommandSubmissionSource {
    UserInterface,
    Api,
}

#[derive(Debug, Clone)]
pub struct CommandSubmission {
    pub agent_id: AgentId,
    pub command: AgentCommand,
    pub idempotency_key: String,
    pub expected_agent_revision: Option<u64>,
    pub requested_model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandAdmissionError {
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
    #[error(transparent)]
    Authority(#[from] CommandAuthorizationError),
    #[error(transparent)]
    Ledger(#[from] CommandLedgerError),
    #[error(transparent)]
    Input(#[from] AgentOperationError),
    #[error("the requested model does not match the current Agent settings")]
    ModelMismatch,
}

pub struct AgentCommandService {
    pub(super) store: Arc<dyn AgentCommandStore>,
    pub(super) access: Arc<ResourceAccessService>,
    authority: Arc<dyn AgentCommandAdmissionAuthority>,
    task_experience: Arc<crate::sandbox::task_experience::TaskExperiencePolicy>,
}

impl AgentCommandService {
    pub fn new(
        store: Arc<dyn AgentCommandStore>,
        access: Arc<ResourceAccessService>,
        authority: Arc<dyn AgentCommandAdmissionAuthority>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            access,
            authority,
            task_experience: Default::default(),
        })
    }

    pub fn with_task_experience(
        self: Arc<Self>,
        policy: Arc<crate::sandbox::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            access: self.access.clone(),
            authority: self.authority.clone(),
            task_experience: policy,
        })
    }

    pub fn task_experience_policy(
        &self,
    ) -> Arc<crate::sandbox::task_experience::TaskExperiencePolicy> {
        self.task_experience.clone()
    }

    /// Revalidate operation visibility before returning an older adapter's
    /// exact receipt. Input validation and execution custody are deliberately
    /// separate: a historical validation/refusal receipt is still replayable.
    pub async fn authorize_intent(
        &self,
        context: &CallerContext,
        credential: &str,
        agent_id: &AgentId,
        command: &AgentCommand,
    ) -> Result<(), CommandAdmissionError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(ResourceAccessError::Access(AccessError::NotFound).into());
        }
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = command_capability(command);
        let authorized = bounded(self.access.authorize(
            context,
            Some(credential),
            &target,
            capability,
            AccessPurpose::Mutation {
                started_at: SystemTime::now(),
            },
        ))
        .await??;
        self.fence(&authorized, &target, capability).await
    }

    /// Current access and durable credential custody precede reservation. No
    /// runtime or lifecycle work occurs in this service; acceptance means the
    /// receipt is durable and the worker can independently revalidate execution.
    pub async fn admit(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: CommandSubmission,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        let (mut authorized, mut request) = self
            .prepare(context, credential, source, submission.clone())
            .await?;
        let target = ResourceTarget::Agent {
            agent_id: request.envelope.agent_id.clone(),
        };
        let capability = command_capability(&request.envelope.command);
        if AnswerContext::from_command(&request.envelope.command)?.is_some()
            && !self.task_experience.allows(&context.caller.subject,
                crate::sandbox::task_experience::TaskExperienceCapability::SuggestedFollowupAdmission)
        {
            let existing = self
                .with_authority_bound(
                    &authorized,
                    capability,
                    self.store.get_admitted_command(&request.scope(), &request.key_sha256),
                )
                .await;
            self.fence(&authorized, &target, capability).await?;
            if let Some(record) = existing?? {
                let CommandReservationResult::Existing(record) = request.replay(record)? else {
                    return Err(CommandLedgerError::Corrupt.into());
                };
                return Ok(CommandStatusView::from_record(&record));
            }
            return Err(ResourceAccessError::Access(AccessError::Forbidden).into());
        }
        let expected = request.clone();
        let lineage = &authorized.snapshot.lineage;
        let retry_creation = submission.expected_agent_revision.is_none()
            && request.binding.is_none()
            && matches!(request.envelope.command, AgentCommand::Message { .. })
            && lineage.agent.as_ref().is_some_and(|agent| {
                lineage.workspace.as_ref().is_some_and(|workspace| {
                    agent
                        .bootstrap_lineage(&lineage.sandbox, workspace)
                        .or_else(|| agent.prepared_creation_lineage(&lineage.sandbox, workspace))
                        .is_some()
                })
            });
        let retry_deadline = request
            .authority_deadline
            .map_or(tokio::time::Instant::now() + IO_BOUND, |deadline| {
                deadline.min(tokio::time::Instant::now() + IO_BOUND)
            });
        for attempt in 0..CREATION_ADMISSION_ATTEMPTS {
            let result = self
                .with_authority_bound(&authorized, capability, self.store.reserve_command(request))
                .await;
            // Admission can be durable even if this final fence refuses the reply.
            // Never refresh past a failed policy fence or an ambiguous result.
            self.fence(&authorized, &target, capability).await?;
            let result = result?;
            if matches!(result, Err(CommandLedgerError::Conflict))
                && retry_creation
                && attempt + 1 < CREATION_ADMISSION_ATTEMPTS
            {
                // A conflict is a noncommitted transaction. Startup workers can
                // advance either aggregate during authority IO; reauthorize the
                // same intent rather than retrying its permanently stale guards.
                expected.require_current_authority()?;
                let (fresh_authorized, mut fresh) = tokio::time::timeout_at(
                    retry_deadline,
                    self.prepare(context, credential, source, submission.clone()),
                )
                .await
                .map_err(|_| CommandLedgerError::Unavailable)??;
                if fresh.scope() != expected.scope() || fresh.policy != expected.policy {
                    return Err(CommandLedgerError::Conflict.into());
                }
                fresh.envelope = AgentCommandEnvelope {
                    authorization: fresh.envelope.authorization,
                    ..expected.envelope.clone()
                };
                fresh.response_id = expected.response_id.clone();
                fresh.admitted_at = expected.admitted_at.clone();
                fresh.queue_deadline = expected.queue_deadline.clone();
                // Fresh evidence cannot extend the original authority or IO budget.
                fresh.authority_deadline = Some(
                    fresh
                        .authority_deadline
                        .map_or(retry_deadline, |deadline| deadline.min(retry_deadline)),
                );
                fresh.validate()?;
                tracing::info!(agent_id = %submission.agent_id, attempt = attempt + 2,
                    reason = "creation_revision_conflict", "Refreshing uncommitted command admission");
                authorized = fresh_authorized;
                request = fresh;
                continue;
            }
            let record = match result? {
                CommandReservationResult::Created(record)
                | CommandReservationResult::Existing(record) => *record,
            };
            let CommandReservationResult::Existing(record) = expected.replay(record)? else {
                return Err(CommandLedgerError::Corrupt.into());
            };
            return Ok(CommandStatusView::from_record(&record));
        }
        Err(CommandLedgerError::Conflict.into())
    }

    /// Recover only an already admitted exact request under fresh mutation
    /// authority. This never reserves a key or creates work for an ambiguous
    /// pre-migration operation whose execution may have happened elsewhere.
    pub async fn lookup_admitted(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: CommandSubmission,
    ) -> Result<Option<CommandStatusView>, CommandAdmissionError> {
        let (authorized, request) = self
            .prepare(context, credential, source, submission)
            .await?;
        let target = ResourceTarget::Agent {
            agent_id: request.envelope.agent_id.clone(),
        };
        let capability = command_capability(&request.envelope.command);
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store
                    .get_admitted_command(&request.scope(), &request.key_sha256),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let Some(record) = result?? else {
            return Ok(None);
        };
        let CommandReservationResult::Existing(record) = request.replay(record)? else {
            return Err(CommandLedgerError::Corrupt.into());
        };
        Ok(Some(CommandStatusView::from_record(&record)))
    }

    pub(super) async fn prepare(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: CommandSubmission,
    ) -> Result<(AuthorizedResource, CommandReservation), CommandAdmissionError> {
        let started_at = SystemTime::now();
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(ResourceAccessError::Access(AccessError::NotFound).into());
        }
        if matches!(
            &context.actor,
            AgentActor::Api {
                delegation_id: Some(_),
                ..
            }
        ) && !matches!(source, CommandSubmissionSource::Api)
        {
            return Err(ResourceAccessError::Access(AccessError::NotFound).into());
        }
        let command = submission.command.canonical()?;
        // Current runtime content support must not silently discard attachments.
        if let AgentCommand::Message { input, .. } | AgentCommand::Steer { input } = &command {
            input.clone().into_text()?;
        }
        let key_sha256 = command_key_digest(&submission.idempotency_key)?;
        if submission.expected_agent_revision == Some(0) {
            return Err(CommandLedgerError::Invalid.into());
        }
        let capability = command_capability(&command);
        let target = ResourceTarget::Agent {
            agent_id: submission.agent_id.clone(),
        };
        let authorized = bounded(self.access.authorize(
            context,
            Some(credential),
            &target,
            capability,
            AccessPurpose::Mutation { started_at },
        ))
        .await??;
        let owner = authorized
            .snapshot
            .scope(&target)
            .map_err(ResourceAccessError::from)?
            .owner;
        let authority = self
            .with_authority_bound(
                &authorized,
                capability,
                self.authority
                    .command_admission_authority(context, credential, &owner),
            )
            .await;
        // Fence authority failures too, so a concurrent policy change cannot be
        // hidden by the result of unrelated provider IO.
        self.fence(&authorized, &target, capability).await?;
        let authority = authority??;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        if submission
            .requested_model
            .as_ref()
            .is_some_and(|requested| agent.selected_model().as_ref() != Some(requested))
        {
            return Err(CommandAdmissionError::ModelMismatch);
        }
        let command_id = AgentCommandId::generate();
        let request = CommandReservation {
            binding: authorized.binding_authority().cloned(),
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: command_id.clone(),
                agent_id: agent.id.clone(),
                owner,
                actor: context.actor.clone(),
                source: AgentCommandSource {
                    kind: match source {
                        CommandSubmissionSource::UserInterface => AgentSourceKind::UserInterface,
                        CommandSubmissionSource::Api => AgentSourceKind::Api,
                    },
                    reference: command_id.to_string(),
                },
                correlation_id: command_id.to_string(),
                request_fingerprint: command.fingerprint()?,
                authorization: authority,
                command: command.clone(),
            },
            key_sha256,
            response_id: matches!(command, AgentCommand::Message { .. })
                .then(AgentResponseId::generate),
            admitted_at: rfc3339_at(started_at),
            queue_deadline: rfc3339_at(started_at + QUEUE_LIFETIME),
            queue_capacity: MAX_COMMAND_QUEUE_CAPACITY,
            expected_agent_revision: submission.expected_agent_revision.unwrap_or(agent.revision),
            parent: SandboxMutationGuard::capture(&authorized.snapshot.lineage.sandbox),
            policy: authorized.snapshot.fence(),
            authority_deadline: match (
                authorized.decision.revalidation_deadline(capability),
                context.credential_deadline(),
            ) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            },
            admission_audit: Some(authorized.authorization_audit(
                &context.actor,
                &target,
                capability,
            )?),
        };
        Ok((authorized, request))
    }

    pub(super) async fn fence(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<(), CommandAdmissionError> {
        self.with_authority_bound(
            authorized,
            capability,
            self.access
                .validate_read_result(authorized, target, capability),
        )
        .await??;
        Ok(())
    }

    pub(super) async fn with_authority_bound<T>(
        &self,
        authorized: &AuthorizedResource,
        capability: ResourceCapability,
        work: impl std::future::Future<Output = T>,
    ) -> Result<T, CommandAdmissionError> {
        authorized
            .decision
            .require(capability)
            .map_err(ResourceAccessError::from)?;
        let deadline = authorized
            .decision
            .revalidation_deadline(capability)
            .map_or(tokio::time::Instant::now() + IO_BOUND, |deadline| {
                deadline.min(tokio::time::Instant::now() + IO_BOUND)
            });
        let result = tokio::time::timeout_at(deadline, work).await;
        authorized
            .decision
            .require(capability)
            .map_err(ResourceAccessError::from)?;
        result.map_err(|_| CommandLedgerError::Unavailable.into())
    }
}

pub(super) async fn bounded<T>(
    work: impl std::future::Future<Output = T>,
) -> Result<T, CommandAdmissionError> {
    tokio::time::timeout(IO_BOUND, work)
        .await
        .map_err(|_| CommandLedgerError::Unavailable.into())
}

pub(super) fn command_capability(command: &AgentCommand) -> ResourceCapability {
    match command {
        AgentCommand::Message { .. } => ResourceCapability::Interact,
        AgentCommand::Steer { .. } | AgentCommand::Interrupt => ResourceCapability::OperateAgent,
        AgentCommand::AnswerApproval { .. } => ResourceCapability::ResolveApproval,
    }
}
