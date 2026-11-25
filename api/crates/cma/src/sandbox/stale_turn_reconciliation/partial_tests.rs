use std::sync::atomic::{AtomicUsize, Ordering};

use layer2_core::agent_ownership::Authority;
use layer2_core::sandbox::{store::AgentStore, DurableAgentUpdateRecorder};
use layer3_adapter::stale_turn::{StaleTurnEvidence, StaleTurnTarget};
use layer3_adapter::ConversationInputSender;

use super::*;

#[derive(Default)]
struct ProducerWithoutReceipt {
    reads: AtomicUsize,
    stops: AtomicUsize,
}

#[async_trait::async_trait]
impl StaleTurnProducer for ProducerWithoutReceipt {
    async fn observe_stale_turn_seal(
        &self,
        _: &StaleTurnTarget,
        _: usize,
    ) -> Result<StaleTurnEvidence, StaleTurnHold> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Err(StaleTurnHold::Unconfirmed)
    }

    async fn request_stale_turn_stop(&self, _: &StaleTurnTarget) -> Result<(), StaleTurnHold> {
        self.stops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

pub(crate) async fn assert_partial_does_not_request_stop(
    store: Arc<MongoSandboxStore>,
    manager: Arc<OwnershipCoordinator>,
    owner: &Arc<Authority>,
    candidate: &Candidate,
    now: DateTime,
) {
    let registry = Arc::new(LiveAgentRegistry::default());
    registry.install_ownership(manager.clone());
    let worker = Worker::new(store.clone(), manager, registry.clone(), None);
    let mut budget = ProofBudget::default();
    let Observation::Quiet {
        target, evidence, ..
    } = worker
        .store
        .observe_for_terminal(candidate, now, &mut budget)
        .await
        .unwrap()
    else {
        panic!("expected a partial candidate eligible for a receipt read");
    };
    let agent = evidence.snapshot.agent.clone();
    let (_send, receive) = tokio::sync::mpsc::channel(1);
    owner
        .run(async {
            let attachment = registry
                .attach(
                    agent.id.clone(),
                    target.conversation_id.clone(),
                    target.runtime_generation,
                    1,
                    agent.capabilities.clone(),
                    ConversationInputSender::spawn(1, |_| async { Ok(()) }),
                    receive,
                    DurableAgentUpdateRecorder::new(store.clone()),
                )
                .unwrap();
            registry.publish_and_activate(&attachment).unwrap();
        })
        .await
        .unwrap();

    let producer = ProducerWithoutReceipt::default();
    assert_eq!(
        worker
            .try_terminal(candidate, now, target, evidence, &mut budget, |_| {
                Some(&producer)
            })
            .await,
        Err("output_uncommitted")
    );
    assert_eq!(producer.reads.load(Ordering::Relaxed), 1);
    assert_eq!(producer.stops.load(Ordering::Relaxed), 0);
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
}
