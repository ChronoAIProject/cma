//! Test composition for real legacy routers and the common command driver.
use crate::AgentConversationState;
use async_trait::async_trait;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
    Router,
};
use layer2_core::{
    resource_access::*,
    sandbox::{
        agent_io::*,
        store::{InMemorySandboxStore, PageRequest},
        AgentOperationError,
    },
    Authorization, Caller, Capability, SubjectId,
};
use std::{sync::Arc, time::Duration};

pub(crate) fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        credential: CredentialContext {
            issuer: "https://fixture.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "exact-fixture".into(),
        },
        memberships: vec![],
    }
}

pub(crate) struct Authority;
#[async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if credential != "fixture-credential"
            || context.caller.subject != SubjectId::new("owner-a")
            || owner
                != &(AgentOwner::Person {
                    subject: context.caller.subject.clone(),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "fixture-custody".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
#[async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::NotMember)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}

pub(crate) fn compose(
    mut state: AgentConversationState,
    store: Arc<InMemorySandboxStore>,
) -> AgentConversationState {
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    state.reads = state.reads.with_resource_access(access.clone());
    state.with_commands(
        AgentCommandService::new(store, access, authority)
            .with_task_experience(crate::agent_v2::fixtures::task_policy()),
    )
}

pub(crate) fn authenticate(request: &mut Request) {
    let Some(subject) = request
        .extensions()
        .get::<crate::AuthenticatedSubject>()
        .map(|value| value.0.clone())
    else {
        return;
    };
    let Some(authorization) = request.extensions().get::<Authorization>().cloned() else {
        return;
    };
    let mut context = context(subject.as_str());
    context.caller.authorization = authorization;
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("fixture-credential".into()));
}

pub(crate) fn reservation_conflict(
    state: &mut AgentConversationState,
    store: Arc<InMemorySandboxStore>,
) {
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    state.commands = Some(AgentCommandService::new(
        Arc::new(ReservationConflict(store)),
        access,
        authority,
    ));
}

struct ReservationConflict(Arc<InMemorySandboxStore>);
#[async_trait]
impl AgentCommandStore for ReservationConflict {
    async fn reserve_command(
        &self,
        _: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        Err(CommandLedgerError::Conflict)
    }
    async fn get_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.0.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &layer2_core::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.0.get_response_command(id).await
    }
}

pub(crate) fn router(state: AgentConversationState, store: Arc<InMemorySandboxStore>) -> Router {
    let execution = Arc::new(Execution(state.clone()));
    let worker =
        AgentCommandWorker::new(store, execution, "protocol-contract-worker".into()).unwrap();
    let v2 = crate::responses::v2::router(crate::responses::v2::ResponsesV2State::new(
        state.commands.clone().unwrap(),
        state.reads.clone(),
    ));
    crate::agui::router(state.clone())
        .merge(crate::responses::router(state))
        .merge(v2)
        .layer(axum::middleware::from_fn_with_state(worker, drive))
}

async fn drive(
    State(worker): State<Arc<AgentCommandWorker>>,
    mut request: Request,
    next: Next,
) -> Response {
    authenticate(&mut request);
    let v2 =
        request.method() == axum::http::Method::POST && request.uri().path() == "/api/v2/responses";
    let mutation = v2
        || request.method() == axum::http::Method::POST
            && matches!(
                request.uri().path(),
                "/api/v1/agui/run" | "/api/v1/responses"
            );
    if !mutation {
        return next.run(request).await;
    }
    if v2 {
        let response = next.run(request).await;
        for command in worker
            .discover(&PageRequest::default())
            .await
            .unwrap()
            .items
        {
            worker
                .run_one(&command.receipt.envelope.command_id)
                .await
                .unwrap();
        }
        return response;
    }
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut interval = Duration::from_millis(10);
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                _ = tokio::time::sleep(interval) => {}
            }
            for command in worker
                .discover(&PageRequest::default())
                .await
                .unwrap()
                .items
            {
                interval = Duration::from_secs(1);
                // Returning response headers never interrupts an executing worker.
                worker
                    .run_one(&command.receipt.envelope.command_id)
                    .await
                    .unwrap();
            }
        }
    });
    let response = next.run(request).await;
    let _ = stop.send(());
    response
}

struct Execution(AgentConversationState);
#[async_trait]
impl AgentCommandExecution for Execution {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        CommandReadiness::Ready
    }
    async fn prepare_queued(&self, _: &AdmittedAgentCommand) {}
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        match self
            .0
            .operations
            .execute_claimed_command(&context("owner-a").caller, command)
            .await
        {
            Ok(_) => CommandAttempt::Finished,
            Err(
                AgentOperationError::PreviousTurnConflict
                | AgentOperationError::ApprovalNotPending { .. }
                | AgentOperationError::NoActiveTurn,
            ) => CommandAttempt::Reject(CommandFailureCode::TargetTurnChanged),
            Err(
                AgentOperationError::CapabilityUnsupported { .. }
                | AgentOperationError::ContentUnsupported
                | AgentOperationError::InvalidRequest(_)
                | AgentOperationError::LogCapacityExceeded,
            ) => CommandAttempt::Reject(CommandFailureCode::ExecutionFailed),
            Err(AgentOperationError::NotFound) => {
                CommandAttempt::Reject(CommandFailureCode::AgentUnavailable)
            }
            Err(AgentOperationError::Forbidden) => {
                CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked)
            }
            Err(_) => CommandAttempt::Retry,
        }
    }
}
