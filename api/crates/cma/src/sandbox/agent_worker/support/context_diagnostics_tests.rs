use super::*;
use crate::sandbox::agent_worker_tests::context_overlap_fixture::missing_runtime;
use layer2_core::agent_ownership::{Authority, OwnershipToken, PROTOCOL_VERSION};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::Instrument;

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

async fn inspection_failure(step: AgentWorkStep, expected_lines: usize) {
    let (worker, agent) = missing_runtime(step).await;
    let authority = Authority::new(
        OwnershipToken {
            agent_id: agent.id.clone(),
            instance_id: "private-inspection-am".into(),
            epoch: 19,
            protocol: PROTOCOL_VERSION,
        },
        tokio::time::Instant::now() + Duration::from_secs(30),
    );
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    assert!(authority
        .run(worker.context(&agent).instrument(tracing::warn_span!(
            "private_inspection",
            private_context = "private-inspection-parent"
        )))
        .await
        .unwrap()
        .is_err());
    let raw = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let events = raw
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|event| {
            event["fields"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("runtime inspection completed")
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), expected_lines);
    let identity = authority.receiver_correlation();
    assert_eq!(
        identity.sandbox_sha256,
        Some(format!(
            "{:x}",
            Sha256::digest(agent.sandbox_id.as_str().as_bytes())
        ))
    );
    for event in events {
        let fields = &event["fields"];
        assert_eq!(fields["succeeded"], false);
        assert_eq!(fields["agent_sha256"], identity.agent_sha256);
        assert_eq!(
            fields["sandbox_sha256"],
            identity.sandbox_sha256.as_ref().unwrap().as_str()
        );
        assert_eq!(fields["instance_sha256"], identity.instance_sha256.as_str());
        assert_eq!(fields["epoch"], 19);
        let line = event.to_string();
        for secret in [
            agent.id.as_str(),
            agent.sandbox_id.as_str(),
            "private-inspection-am",
            "private-inspection-parent",
        ] {
            assert!(!line.contains(secret), "inspection leaked a raw identifier");
        }
        assert!(fields.get("agent_id").is_none());
        assert!(fields.get("sandbox_id").is_none());
    }
}

#[tokio::test]
async fn ordinary_inspection_failure_has_joinable_hashed_identity() {
    inspection_failure(AgentWorkStep::AddWorktree, 1).await;
}

#[tokio::test]
async fn overlapped_inspection_failure_has_joinable_hashed_identity() {
    inspection_failure(AgentWorkStep::OpenConversation, 2).await;
}
