use super::*;
use crate::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipStore,
    DEFAULT_MAX_OWNED_AGENTS, PROTOCOL_VERSION,
};
use tokio::sync::Notify;

async fn manager(store: Arc<MemoryOwnershipStore>, instance: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: instance.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://fixture.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
    )
    .await
    .unwrap()
}

#[derive(Clone, Copy)]
enum Form {
    Ordinary,
    Targeted,
    Public,
    Pair,
}

impl Form {
    async fn admit(
        self,
        service: &AgentCommandService,
        request: TargetedControlSubmission,
    ) -> Result<crate::ids::AgentCommandId, String> {
        let caller = context("owner");
        let credential = "selected-credential";
        let source = CommandSubmissionSource::Api;
        let result = match self {
            Self::Ordinary => service
                .admit(
                    &caller,
                    credential,
                    source,
                    CommandSubmission {
                        agent_id: request.agent_id,
                        command: AgentCommand::Interrupt,
                        idempotency_key: request.idempotency_key,
                        expected_agent_revision: None,
                        requested_model: None,
                    },
                )
                .await
                .map_err(|error| error.to_string())?,
            Self::Targeted => {
                service
                    .admit_targeted_control(&caller, credential, source, request)
                    .await
                    .map_err(|error| error.to_string())?
                    .control
            }
            Self::Public => {
                service
                    .admit_response_control(&caller, credential, source, request)
                    .await
                    .map_err(|error| error.to_string())?
                    .control
            }
            Self::Pair => {
                service
                    .interrupt_with_message(
                        &caller,
                        credential,
                        source,
                        crate::sandbox::agent_io::InterruptingMessageSubmission {
                            agent_id: request.agent_id,
                            target: request.target,
                            expected_agent_revision: request.expected_agent_revision,
                            text: "Synthetic follow-up".into(),
                            idempotency_key: request.idempotency_key,
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())?
                    .control
            }
        };
        Ok(result.receipt.command_id)
    }
}

async fn assert_no_wake(wake: &Notify) {
    assert!(tokio::time::timeout(Duration::ZERO, wake.notified())
        .await
        .is_err());
}

#[tokio::test]
async fn confirmed_cancel_wakes_once_for_every_admission_form() {
    for form in [Form::Ordinary, Form::Targeted, Form::Public, Form::Pair] {
        let (store, service, _, request) = fixture().await;
        let owners = Arc::new(MemoryOwnershipStore::default());
        let owner = manager(owners, "cancel-owner").await;
        assert!(owner.acquire(&request.agent_id).await.unwrap().is_some());
        let wake = Arc::new(Notify::new());
        let service = service
            .with_ownership(Some(owner))
            .with_cancel_wake(Some(wake.clone()));

        let id = form.admit(&service, request.clone()).await.unwrap();
        tokio::time::timeout(Duration::ZERO, wake.notified())
            .await
            .unwrap();
        assert!(store.get_command(&id).await.unwrap().is_some());
        assert_eq!(form.admit(&service, request).await.unwrap(), id);
        assert_no_wake(&wake).await;
    }
}

#[tokio::test]
async fn non_owner_and_lost_owner_cannot_publish_a_cancel_wake() {
    let (store, service, _, mut request) = fixture().await;
    let owners = Arc::new(MemoryOwnershipStore::default());
    let first = manager(owners.clone(), "first-cancel-owner").await;
    let authority = first.acquire(&request.agent_id).await.unwrap().unwrap();
    let second = manager(owners.client(), "second-cancel-owner").await;
    let wake = Arc::new(Notify::new());
    let non_owner = service
        .clone()
        .with_ownership(Some(second.clone()))
        .with_cancel_wake(Some(wake.clone()));
    let admitted = Form::Ordinary
        .admit(&non_owner, request.clone())
        .await
        .unwrap();
    assert_eq!(
        owners
            .owner(&request.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        authority.token,
    );
    assert_no_wake(&wake).await;

    first.begin_drain().await;
    first.handoff(&authority, async { Ok(()) }).await.unwrap();
    assert!(second.acquire(&request.agent_id).await.unwrap().is_some());
    let former_owner = service
        .with_ownership(Some(first))
        .with_cancel_wake(Some(wake.clone()));
    request.idempotency_key = "after-owner-handoff".into();
    let another = Form::Ordinary.admit(&former_owner, request).await.unwrap();
    assert_ne!(admitted, another);
    assert!(store.get_command(&another).await.unwrap().is_some());
    assert_no_wake(&wake).await;
}

struct RefuseReservation {
    store: Arc<InMemorySandboxStore>,
    commit_before_error: bool,
}

#[async_trait]
impl AgentCommandStore for RefuseReservation {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        if self.commit_before_error {
            self.store.reserve_command(request).await?;
            return Err(CommandLedgerError::Unavailable);
        }
        Err(CommandLedgerError::Conflict)
    }

    async fn get_command(
        &self,
        id: &crate::ids::AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.store.get_command(id).await
    }

    async fn get_response_command(
        &self,
        id: &crate::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.store.get_response_command(id).await
    }
}

#[tokio::test]
async fn abort_and_uncertain_commit_do_not_signal_discovery() {
    for commit_before_error in [false, true] {
        let (store, service, credentials, request) = fixture().await;
        let owners = Arc::new(MemoryOwnershipStore::default());
        let owner = manager(owners, "cancel-owner").await;
        assert!(owner.acquire(&request.agent_id).await.unwrap().is_some());
        let wake = Arc::new(Notify::new());
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            credentials.clone(),
        ));
        let failing = AgentCommandService::new(
            Arc::new(RefuseReservation {
                store,
                commit_before_error,
            }),
            access,
            credentials,
        )
        .with_ownership(Some(owner.clone()))
        .with_cancel_wake(Some(wake.clone()));
        assert!(Form::Ordinary
            .admit(&failing, request.clone())
            .await
            .is_err());
        assert_no_wake(&wake).await;
        let recovered = service
            .with_ownership(Some(owner))
            .with_cancel_wake(Some(wake.clone()));
        Form::Ordinary.admit(&recovered, request).await.unwrap();
        if commit_before_error {
            assert_no_wake(&wake).await;
        } else {
            tokio::time::timeout(Duration::ZERO, wake.notified())
                .await
                .unwrap();
        }
    }
}
