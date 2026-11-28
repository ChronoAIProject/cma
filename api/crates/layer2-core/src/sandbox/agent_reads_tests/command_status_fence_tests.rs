use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use std::collections::VecDeque;

struct FailingSnapshots {
    inner: Arc<InMemorySandboxStore>,
    errors: Mutex<VecDeque<ResourceAccessStoreError>>,
    reads: AtomicUsize,
}

#[async_trait]
impl ResourceAccessStore for FailingSnapshots {
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("response fence does not list Sandboxes")
    }

    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("response fence does not list Sandbox shares")
    }

    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &crate::sandbox::AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("response fence does not list Agents")
    }

    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let error = self.errors.lock().unwrap().pop_front();
        match error {
            Some(error) => Err(error),
            None => self.inner.resource_access_snapshot(target).await,
        }
    }
}

#[async_trait]
impl ResourceShareStore for FailingSnapshots {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        unreachable!("response fence does not replay share mutations")
    }

    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        unreachable!("response fence does not change shares")
    }

    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        unreachable!("response fence reads shares in its snapshot")
    }
}

#[tokio::test]
async fn response_stream_retries_only_unavailable_authorization_snapshots() {
    use ResourceAccessStoreError::{Conflict, Unavailable};

    let (store, commands, authority, agent) = setup(Authority::new()).await;
    let receipt = commands
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("response-fence"),
        )
        .await
        .unwrap()
        .receipt;
    let id = receipt.response_id.as_ref().unwrap();
    let reads = service(store.clone()).with_resource_access(Arc::new(ResourceAccessService::new(
        store.clone(),
        authority.clone(),
    )));
    let share_record = share(&store, None).await;
    let reader = reads
        .authorize_event_stream(&context("reader"), Some("selected-credential"), &agent.id)
        .await
        .unwrap();
    share(&store, Some(share_record)).await;
    let owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();

    let access = Arc::new(FailingSnapshots {
        inner: store.clone(),
        errors: Mutex::new(VecDeque::from([Unavailable])),
        reads: AtomicUsize::new(0),
    });
    let commands = AgentCommandService::new(
        store,
        Arc::new(ResourceAccessService::new(
            access.clone(),
            authority.clone(),
        )),
        authority,
    );
    let result = commands.response_for_stream(&owner, id).await.unwrap();
    assert_eq!(result.command.receipt, receipt);
    assert_eq!(access.reads.swap(0, Ordering::SeqCst), 2);

    for (held, errors, expected, reads) in [
        (&reader, vec![Unavailable], Conflict, 2),
        (&owner, vec![Unavailable, Unavailable], Unavailable, 2),
        (&owner, vec![Conflict], Conflict, 1),
    ] {
        *access.errors.lock().unwrap() = VecDeque::from(errors);
        let err = commands.response_for_stream(held, id).await.unwrap_err();
        assert!(matches!(
            err,
            CommandAdmissionError::Access(ResourceAccessError::Store(actual))
                if actual == expected
        ));
        assert_eq!(access.reads.swap(0, Ordering::SeqCst), reads);
    }
}

struct BufferedStore {
    inner: Arc<InMemorySandboxStore>,
    calls: AtomicUsize,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    error: bool,
}
#[async_trait]
impl AgentCommandStore for BufferedStore {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.inner.reserve_command(request).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let value = self.inner.get_command(id).await;
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.entered.notify_one();
            self.release.notified().await;
            if self.error {
                return Err(CommandLedgerError::Unavailable);
            }
        }
        value
    }
}

#[tokio::test]
async fn revoked_policy_overrides_buffered_status_and_dependency_errors() {
    for (error, projection) in [(false, false), (true, false), (false, true), (true, true)] {
        let (store, service, authority, _) = setup(Authority::new()).await;
        let receipt = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("one"),
            )
            .await
            .unwrap();
        let grant = share(&store, None).await;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let buffered = Arc::new(BufferedStore {
            inner: store.clone(),
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
            error,
        });
        let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
        let reads = AgentCommandService::new(buffered, access, authority);
        let work = tokio::spawn(async move {
            if projection {
                reads
                    .command_projection(
                        &context("reader"),
                        Some("selected-credential"),
                        &receipt.receipt.command_id,
                    )
                    .await
                    .map(|value| value.status)
            } else {
                reads
                    .command_status(
                        &context("reader"),
                        Some("selected-credential"),
                        &receipt.receipt.command_id,
                    )
                    .await
            }
        });
        entered.notified().await;
        share(&store, Some(grant)).await;
        release.notify_one();
        assert!(matches!(
            work.await.unwrap(),
            Err(CommandAdmissionError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            )))
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn held_membership_expiry_cancels_buffered_status_instead_of_renewing_it() {
    for projection in [false, true] {
        let mut authority = Authority::new();
        authority.lifetime = Duration::from_secs(1);
        let (store, service, authority, _) = setup(authority).await;
        let receipt = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("one"),
            )
            .await
            .unwrap();
        share(&store, None).await;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let buffered = Arc::new(BufferedStore {
            inner: store.clone(),
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release,
            error: false,
        });
        let access = Arc::new(ResourceAccessService::new(store, authority.clone()));
        let reads = AgentCommandService::new(buffered, access, authority);
        let work = tokio::spawn(async move {
            if projection {
                reads
                    .command_projection(
                        &context("reader"),
                        Some("selected-credential"),
                        &receipt.receipt.command_id,
                    )
                    .await
                    .map(|value| value.status)
            } else {
                reads
                    .command_status(
                        &context("reader"),
                        Some("selected-credential"),
                        &receipt.receipt.command_id,
                    )
                    .await
            }
        });
        entered.notified().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(
            work.await.unwrap(),
            Err(CommandAdmissionError::Access(ResourceAccessError::Access(
                AccessError::RevalidationRequired
            )))
        ));
    }
}
