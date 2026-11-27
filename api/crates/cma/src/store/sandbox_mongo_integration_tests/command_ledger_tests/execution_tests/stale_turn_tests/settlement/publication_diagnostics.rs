use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use layer2_core::{
    ids::AgentCommandId,
    sandbox::{store::AgentLogStore, AgentLogEntry, AgentLogKind},
    AgentId,
};
use mongodb::bson::doc;
use tracing_subscriber::prelude::*;

use crate::store::MongoSandboxStore;

const CONFIRMED: &str = "Stale turn publication confirmed";

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

pub(super) struct Log {
    output: Output,
    _guard: tracing::subscriber::DefaultGuard,
}

impl Log {
    pub(super) fn capture() -> Self {
        let output = Output::default();
        let sink = output.clone();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(move || sink.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                meta.target().ends_with("::stale_turns::settlement")
            }));
        let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
        Self {
            output,
            _guard: guard,
        }
    }

    fn confirmations(&self) -> Vec<serde_json::Value> {
        let bytes = self.output.0.lock().unwrap();
        std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .map(|event| event["fields"].clone())
            .filter(|fields| fields["message"] == CONFIRMED)
            .collect()
    }

    pub(super) fn assert_empty(&self) {
        assert!(self.confirmations().is_empty());
    }

    pub(super) async fn assert_terminal(
        &self,
        store: &MongoSandboxStore,
        agent: &AgentId,
        id: &AgentCommandId,
        ambiguous: bool,
    ) {
        let logs = store.agent_log(agent, 0, 100).await.unwrap();
        let mut terminal = logs.iter().filter(|entry| is_stale_turn_failure(entry));
        let terminal_sequence = terminal.next().unwrap().seq;
        assert!(terminal.next().is_none());
        assert_eq!(logs.iter().filter(|entry| {
            matches!(&entry.kind, AgentLogKind::AgentMessage { content } if content == "retained prefix")
        }).count(), 1);

        let record = store
            .agent_commands
            .find_one(doc! { "_id": id.as_str() })
            .projection(doc! { "stale_turn_publication.terminal_seq": 1 })
            .await
            .unwrap()
            .unwrap();
        let receipt = record.get_document("stale_turn_publication").unwrap();
        let receipt_sequence = u64::try_from(receipt.get_i64("terminal_seq").unwrap()).unwrap();
        assert_eq!(receipt_sequence, terminal_sequence);

        let publications = self.confirmations();
        for fields in &publications {
            assert_eq!(fields["command_id"], id.as_str());
            assert_eq!(fields["terminal_seq"], receipt_sequence);
        }
        let confirmation = if ambiguous { "reconciled" } else { "committed" };
        assert_eq!(
            publications
                .iter()
                .filter(|event| event["confirmation"] == confirmation)
                .count(),
            1
        );
        assert!(publications
            .iter()
            .any(|event| event["confirmation"] == "existing_receipt"));
        assert!(publications.iter().all(|event| {
            event["confirmation"] == confirmation || event["confirmation"] == "existing_receipt"
        }));
    }
}

fn is_stale_turn_failure(entry: &AgentLogEntry) -> bool {
    matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_turn_no_progress")
}
