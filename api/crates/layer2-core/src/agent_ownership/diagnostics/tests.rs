use super::*;
use crate::agent_ownership::{OwnershipToken, PROTOCOL_VERSION};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

fn authority(id: &str, instance: &str) -> Arc<Authority> {
    Authority::new(
        OwnershipToken {
            agent_id: AgentId(id.into()),
            instance_id: instance.into(),
            epoch: 7,
            protocol: PROTOCOL_VERSION,
        },
        tokio::time::Instant::now() + Duration::from_secs(30),
    )
}

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn lineage_is_exact_agent_first_observation_and_retained_after_cancellation() {
    let authority = authority("private-agent-one", "private-am-one");
    let sandbox = SandboxId("private-sandbox-one".into());
    observe_agent_sandbox(&authority.token.agent_id, &sandbox);
    assert!(authority.log_identity().sandbox_sha256.is_none());
    authority
        .run(async {
            observe_agent_sandbox(&AgentId("foreign-agent".into()), &sandbox);
            assert!(authority.log_identity().sandbox_sha256.is_none());
            observe_agent_sandbox(&authority.token.agent_id, &sandbox);
            observe_agent_sandbox(
                &authority.token.agent_id,
                &SandboxId("later-sandbox".into()),
            );
        })
        .await
        .unwrap();
    authority.revoke();
    assert_eq!(
        authority.log_identity().sandbox_sha256,
        Some(digest(sandbox.as_str()))
    );
    assert!(!authority.is_current());
}

#[tokio::test]
async fn same_epoch_on_different_agents_or_instances_is_distinguishable() {
    let first = authority("private-agent-one", "private-am-one");
    let second = authority("private-agent-two", "private-am-two");
    let a = first.log_identity();
    let b = second.log_identity();
    assert_eq!(a.epoch, b.epoch);
    assert_ne!(a.agent_sha256, b.agent_sha256);
    assert_ne!(a.instance_sha256, b.instance_sha256);
    assert_eq!(a.agent_sha256.len(), 64);
    assert_eq!(a.instance_sha256.unwrap().len(), 64);
    assert!(a.sandbox_sha256.is_none());
}

struct Refusal;
#[async_trait::async_trait]
impl ReceiverRevocation for Refusal {
    fn identity(&self) -> String {
        "private-runtime".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        Err(layer3_adapter::AdapterError::Unavailable(
            "private-runtime".into(),
        ))
    }
}

#[tokio::test]
async fn revocation_warning_joins_inspection_identity_without_raw_ids_or_losing_receiver() {
    let authority = authority("private-agent-one", "private-am-one");
    let sandbox = SandboxId("private-sandbox-one".into());
    let identity = authority
        .run(async { ReceiverLogIdentity::for_agent(&authority.token.agent_id, &sandbox, None) })
        .await
        .unwrap();
    authority.register_receiver(Arc::new(Refusal)).unwrap();
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let error = authority.revoke_transports().await.unwrap_err();
    authority.log_identity().warn(
        "cancelled_receiver_cleanup",
        &error,
        "Receiver cleanup pending; healthy ownership continues",
    );
    let raw = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let event: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    let fields = &event["fields"];
    assert_eq!(fields["agent_sha256"], identity.agent_sha256);
    assert_eq!(fields["sandbox_sha256"], identity.sandbox_sha256.unwrap());
    assert_eq!(fields["instance_sha256"], identity.instance_sha256.unwrap());
    assert_eq!(fields["epoch"], 7);
    assert_eq!(fields["stage"], "cancelled_receiver_cleanup");
    assert!(fields["error"]
        .as_str()
        .unwrap()
        .contains("receiving runtime revocation failed"));
    assert!(!raw.contains("private-"));
    assert_eq!(authority.receivers.lock().unwrap().len(), 1);
    assert!(!authority.is_current());
}

#[tokio::test]
async fn unobserved_lineage_is_explicitly_unknown() {
    let authority = authority("private-agent-one", "private-am-one");
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    authority.log_identity().warn(
        "cancelled_owner_observation",
        &OwnershipError::Stale,
        "retained",
    );
    let raw = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let event: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(event["fields"]["sandbox_sha256"], "unknown");
    assert!(!raw.contains("private-"));
}
