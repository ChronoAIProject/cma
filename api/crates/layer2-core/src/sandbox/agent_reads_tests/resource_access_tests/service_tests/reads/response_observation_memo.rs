use super::*;
use crate::sandbox::agent_observation::*;

struct Facts {
    store: Arc<InMemorySandboxStore>,
    generation: Option<Arc<Generation>>,
}

#[derive(Default)]
struct Generation {
    value: AtomicUsize,
    reads: AtomicUsize,
    release: tokio::sync::Notify,
}

#[async_trait]
impl AgentObservationStore for Facts {
    fn ready(&self) -> bool {
        true
    }
    async fn source_generation(&self, agent: &Agent) -> Result<Option<Vec<u8>>, SandboxStoreError> {
        Ok(Some(serde_json::to_vec(agent).unwrap()))
    }
    async fn output_generation(&self, agent: &Agent) -> Result<Option<Vec<u8>>, SandboxStoreError> {
        let Some(generation) = &self.generation else {
            return self.source_generation(agent).await;
        };
        let value = generation.value.load(Ordering::SeqCst);
        if generation.reads.fetch_add(1, Ordering::SeqCst) == 1 {
            generation.release.notified().await;
        }
        Ok(Some(value.to_le_bytes().to_vec()))
    }
    async fn fresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<Result<ResourceAccessSnapshot, AgentReadError>>, AgentReadError> {
        let mut snapshots = Vec::new();
        for token in tokens {
            snapshots.push(
                self.store
                    .resource_access_snapshot(&ResourceTarget::Agent {
                        agent_id: token.agent_id.clone(),
                    })
                    .await
                    .map_err(|error| AgentReadError::Access(error.into())),
            );
        }
        Ok(snapshots)
    }
    async fn refresh(
        &self,
        _: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn combined_memo_fallback_releases_the_indexed_flight_and_keeps_raw_redaction() {
    for mode in ["ready", "fallback"] {
        let mut f = counted(mode, true).await;
        let observer = AgentObservationService::new(
            f.reads.clone(),
            Arc::new(Facts {
                store: f.store.clone(),
                generation: None,
            }),
        );
        f.reads = f.reads.with_observation(observer);
        let custody = AgentObservationAuthority::new(
            AgentObservationToken {
                agent_id: f.agent.id.clone(),
                created_at: f.agent.created_at.clone(),
                instance_id: "test-replica".into(),
                epoch: 1,
            },
            tokio::time::Instant::now() + Duration::from_secs(15),
        );
        let mut subscriber = f.held.clone();
        let subscription = observation_scope(custody, async {
            f.reads
                .observe_output_stream(&mut subscriber)
                .unwrap()
                .unwrap()
        })
        .await;
        let owner = f
            .reads
            .authorize_event_stream(&context("owner"), None, &f.agent.id)
            .await
            .unwrap();
        f.access.calls.store(0, Ordering::SeqCst);
        let AgentResponseObservation::Legacy(shared) = f.observe().await.unwrap() else {
            panic!("legacy")
        };
        let AgentResponseObservation::Legacy(personal) = f
            .reads
            .with_response_observation(&owner, 3, |_| Box::pin(async { Ok(()) }))
            .await
            .unwrap()
        else {
            panic!("legacy")
        };
        assert!(!serde_json::to_string(&shared.history)
            .unwrap()
            .contains("private-"));
        assert!(serde_json::to_string(&personal.history)
            .unwrap()
            .contains("private-"));
        assert_eq!(f.source.indexed_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            f.source.calls.load(Ordering::SeqCst),
            if mode == "ready" { 1 } else { 2 }
        );
        assert_eq!(
            f.logs.calls.load(Ordering::SeqCst),
            if mode == "ready" { 0 } else { 2 }
        );
        assert_eq!(f.access.calls.load(Ordering::SeqCst), 4);
        if mode == "ready" {
            assert!(shared.source_generation.is_some());
            assert_eq!(shared.source_generation, personal.source_generation);
        }
        drop(subscription);
    }
}

#[tokio::test]
async fn output_generation_snapshot_only_serves_readers_waiting_before_it_starts() {
    let f = counted("ready", true).await;
    let generation = Arc::new(Generation::default());
    let observer = AgentObservationService::new(
        f.reads.clone(),
        Arc::new(Facts {
            store: f.store.clone(),
            generation: Some(generation.clone()),
        }),
    );
    let reads = f.reads.clone().with_observation(observer.clone());
    let custody = AgentObservationAuthority::new(
        AgentObservationToken {
            agent_id: f.agent.id.clone(),
            created_at: f.agent.created_at.clone(),
            instance_id: "test-replica".into(),
            epoch: 1,
        },
        tokio::time::Instant::now() + Duration::from_secs(15),
    );
    let mut subscriber = f.held.clone();
    let _subscription = observation_scope(custody, async {
        reads
            .observe_output_stream(&mut subscriber)
            .unwrap()
            .unwrap()
    })
    .await;

    let active = observer
        .output_flight(&f.agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    let mut first = Box::pin(observer.output_flight(&f.agent, Some(3)));
    let mut waiting = Box::pin(observer.output_flight(&f.agent, Some(3)));
    assert!(futures_util::poll!(&mut first).is_pending());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    drop(active);
    assert!(futures_util::poll!(&mut first).is_pending());
    assert_eq!(generation.reads.load(Ordering::SeqCst), 2);

    // Advance the source's published evidence without changing the Agent.
    generation.value.store(1, Ordering::SeqCst);
    let mut late = Box::pin(observer.output_flight(&f.agent, Some(3)));
    assert!(futures_util::poll!(&mut late).is_pending());
    generation.release.notify_one();
    let first_identity = first.await.unwrap().unwrap().identity();
    let waiting_identity = waiting.await.unwrap().unwrap().identity();
    let late_identity = late.await.unwrap().unwrap().identity();
    assert_eq!(first_identity, waiting_identity);
    assert_ne!(first_identity, late_identity);
    assert_eq!(generation.reads.load(Ordering::SeqCst), 3);
}
