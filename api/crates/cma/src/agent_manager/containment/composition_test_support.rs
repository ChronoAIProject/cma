use super::*;
use crate::config::{AgentManagerReplicas, FeatureFlags};

pub(super) async fn configured(
    combined: bool,
    receiver_only: bool,
) -> (mongodb::Client, String, Arc<AgentManager>, AgentId) {
    let (client, name, store) = database().await;
    let (_, agent) = fixture(&store, "composition-agent").await;
    let store = Arc::new(
        store
            .enable_agent_ownership("composition-member".into())
            .await
            .unwrap(),
    );
    let features = FeatureFlags::from_lookup(&mut |key| {
        Ok(match key {
            "CHRONO_AGENT_MANAGER_FAILURE_CONTAINMENT_ENABLED" => Some(combined.to_string()),
            "CHRONO_RECEIVER_HOUSEKEEPING_CONTAINMENT_ENABLED" => Some(receiver_only.to_string()),
            "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED" => Some("false".into()),
            _ => None,
        })
    })
    .unwrap();
    let manager = AgentManager::register_configured(
        store,
        Arc::new(LiveAgentRegistry::default()),
        &AgentManagerReplicas {
            relay_address: "http://manager.invalid".into(),
            max_owned_agents: 2,
            skip_ineligible_reconciliation: false,
            defer_foreign_reconciliation: false,
        },
        &features,
    )
    .await
    .unwrap();
    manager
        .register_lifecycle(crate::agent_lifecycle::LifecycleDrain::fixture(1))
        .unwrap();
    assert!(!manager.coordinator.is_registered());
    assert!(manager.supervisor.lock().await.is_none());
    assert_eq!(manager.coordinator.contains_failures(), combined);
    (client, name, manager, agent.id)
}

pub(super) struct ClassifiedReceiver {
    pub failure: Mutex<Option<ReceiverSettlementFailure>>,
    pub legacy_calls: AtomicUsize,
    pub typed_calls: AtomicUsize,
}

impl ClassifiedReceiver {
    pub fn install(authority: &Authority, failure: Option<ReceiverSettlementFailure>) -> Arc<Self> {
        let receiver = Arc::new(Self {
            failure: Mutex::new(failure),
            legacy_calls: AtomicUsize::new(0),
            typed_calls: AtomicUsize::new(0),
        });
        authority.register_receiver(receiver.clone()).unwrap();
        authority
            .begin_receiver_confirmation(&receiver.identity())
            .unwrap();
        authority.receiver_confirmed(&receiver.identity());
        receiver
    }
}

#[async_trait::async_trait]
impl ReceiverRevocation for ClassifiedReceiver {
    fn identity(&self) -> String {
        "composition-receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.legacy_calls.fetch_add(1, Ordering::AcqRel);
        if self.failure.lock().unwrap().is_some() {
            Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
    async fn settle_typed(
        &self,
        _: ReceiverConfirmationCustody,
        watch: &ReceiverSettlementWatch,
    ) -> Result<ReceiverCleanup, ReceiverSettlementFailure> {
        self.typed_calls.fetch_add(1, Ordering::AcqRel);
        if let Some(failure) = *self.failure.lock().unwrap() {
            watch.record(failure);
            Err(failure)
        } else {
            Ok(ReceiverCleanup::Acknowledged)
        }
    }
}

pub(super) struct UnexpectedUpdate;
#[async_trait::async_trait]
impl layer2_core::sandbox::AgentUpdateRecorder for UnexpectedUpdate {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut layer2_core::sandbox::AgentMessageAssembly,
        _: layer3_adapter::ConversationUpdate,
        _: String,
    ) -> Result<
        layer2_core::sandbox::AgentUpdateOutcome,
        layer2_core::sandbox::AgentConversationError,
    > {
        panic!("fixture sends no updates")
    }
}
