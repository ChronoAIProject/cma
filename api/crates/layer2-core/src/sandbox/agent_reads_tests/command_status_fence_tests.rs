use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};

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
