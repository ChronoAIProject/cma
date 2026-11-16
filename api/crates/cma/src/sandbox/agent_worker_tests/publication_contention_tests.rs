//! Regression tests at real worker entrypoints, with publication-only faults.
//! This module also applies to cc7e8acd so the package can prove native RED.
use super::*;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::AgentSkillInstallStatus;

mod fixture;
mod store;
use fixture::*;
use store::{Boundary, Change, Fault, PublicationStore};

mod failure;
mod skills;
mod wait;

fn attempts(store: &PublicationStore) -> usize {
    store.attempts.load(Ordering::SeqCst)
}

async fn preserves_metadata(h: &Harness, id: &AgentId, change: Change) {
    let agent = h.stored(id).await;
    let logs = h.store.agent_log(id, 0, 100).await.unwrap();
    assert_eq!(agent.conversation.last_seq, logs.len() as u64);
    for (index, log) in logs.iter().enumerate() {
        assert_eq!(log.seq, index as u64 + 1);
    }
    if matches!(change, Change::Title) {
        assert_eq!(agent.title.as_deref(), Some("Concurrent owner title"));
    }
    if matches!(change, Change::Log) {
        assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::TitleSet { title, .. } if title == "Concurrent transcript title")).count(), 1);
    }
}

const INCOMPATIBLE: &[Change] = &[
    Change::Holder,
    Change::Fence,
    Change::Step,
    Change::Runtime,
    Change::Stop,
    Change::ParentPause,
    Change::ParentRuntime,
    Change::Workspace,
    Change::Worktree,
];
