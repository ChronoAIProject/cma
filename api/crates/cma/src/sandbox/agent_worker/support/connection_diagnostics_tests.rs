use super::*;
use crate::agent_manager::drain_receiver_for_test;
use crate::sandbox::agent_worker_tests::context_overlap_fixture::unbound_worker;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipStore,
    DEFAULT_MAX_OWNED_AGENTS, PROTOCOL_VERSION,
};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tracing_subscriber::prelude::*;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Receiver(AtomicBool);

#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "synthetic-receiver-marker".into()
    }

    async fn revoke(&self) -> layer3_adapter::Result<()> {
        if self.0.load(Ordering::Relaxed) {
            Err(AdapterError::Unavailable("synthetic-error-marker".into()))
        } else {
            Ok(())
        }
    }
}

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

#[tokio::test(start_paused = true)]
async fn inspection_cleanup_and_drain_join_by_hash_across_owner_handoff() {
    let (worker, agent) = unbound_worker().await;
    let output = Output::default();
    let sink = output.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .without_time()
            .event_format(crate::diagnostic_format::json())
            .with_writer(move || sink.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.is_span() || metadata.target() == "receiver_ownership"
            })),
    );
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!("ambient", details = "synthetic-ancestor-marker");
    let _entered = span.enter();
    let store = Arc::new(MemoryOwnershipStore::default());
    let digest = |identity: &str| format!("{:x}", Sha256::digest(identity.as_bytes()));
    let instances = ["synthetic-owner-one", "synthetic-owner-two"];
    let mut epochs = Vec::new();
    for instance in instances {
        let manager = manager(store.client(), instance).await;
        let owner = manager.acquire(&agent.id).await.unwrap().unwrap();
        epochs.push(owner.token.epoch);
        let receiver = Arc::new(Receiver(AtomicBool::new(true)));
        owner.register_receiver(receiver.clone()).unwrap();
        let inspection = owner.run(worker.context(&agent)).await.unwrap();
        assert!(inspection.is_err());
        assert!(drain_receiver_for_test(
            &manager,
            &owner,
            owner.revoke_transports_after_local_drain()
        )
        .await
        .is_err());
        assert_eq!(
            store.owner(&agent.id).await.unwrap().unwrap().token,
            owner.token
        );
        receiver.0.store(false, Ordering::Relaxed);
        drain_receiver_for_test(
            &manager,
            &owner,
            owner.revoke_transports_after_local_drain(),
        )
        .await
        .unwrap();
        manager.stop_admission();
        // Eligibility for placement is read from membership, not the local flag.
        store.heartbeat(instance, true).await.unwrap();
    }
    assert!(epochs[1] > epochs[0]);
    let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    for raw in instances.into_iter().chain([
        agent.id.as_str(),
        agent.sandbox_id.as_str(),
        "synthetic-receiver-marker",
        "synthetic-error-marker",
        "synthetic-ancestor-marker",
    ]) {
        assert!(!captured.contains(raw), "{raw} appeared in {captured}");
    }
    let fields: Vec<_> = captured
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["fields"].clone())
        .collect();
    assert_eq!(fields.len(), 6, "{captured}");
    for (instance, epoch) in instances.into_iter().zip(epochs) {
        let instance_hash = digest(instance);
        let events: Vec<_> = fields
            .iter()
            .filter(|event| event["instance_sha256"] == instance_hash)
            .collect();
        assert_eq!(events.len(), 3);
        for event in &events {
            assert_eq!(event["agent_sha256"], digest(agent.id.as_str()));
            assert_eq!(event["sandbox_sha256"], digest(agent.sandbox_id.as_str()));
            assert_eq!(
                event["receiver_sha256"],
                digest("synthetic-receiver-marker")
            );
        }
        assert_eq!(events[0]["message"], "Agent runtime inspection completed");
        assert_eq!(events[0]["succeeded"], false);
        assert_eq!(events[0]["reason"], "runtime_unavailable");
        for event in &events[1..] {
            assert_eq!(event["stage"], "receiver_revocation");
            assert_eq!(event["reason"], "transport_unavailable");
            assert_eq!(event["epoch"], epoch);
        }
    }
}
