//! Real Mongo admission and worker regressions through two public HTTP ingresses.
use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use crate::store::{MongoOperationStore, MongoSandboxStore};
use axum::{http::Method, Router};
use layer2_core::agent_ownership::{
    candidates, Authority, Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use layer2_core::sandbox::{
    AgentReadService, AgentRuntimeConnection, AgentService, SandboxResourceReadError,
    SandboxResourceReader, SandboxResourceReading,
};
use layer3_adapter::owner_protocol::{
    OwnerAcknowledgement, TransportAuthority, TransportOwner, OWNER_PROTOCOL_VERSION,
};
use mongodb::{bson::doc, Client};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

mod boundaries;
mod cases;
mod idempotency;
mod measurements;
mod observations;
#[cfg(feature = "qualification-hooks")]
mod qualification;
mod receiver_fixture;
mod replay;
mod routes;
mod setup;
mod unknown_commit;
use receiver_fixture::receiver;

struct Registry {
    cancel_authority: Arc<Authority>,
    live: Arc<LiveAgentRegistry>,
    cancel_after_fence: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        AgentRuntimeRegistry::resolve(self.live.as_ref(), target).await
    }
    async fn fence(&self, id: &AgentId, fence: u64) -> Result<(), AgentRuntimeError> {
        AgentRuntimeRegistry::fence(self.live.as_ref(), id, fence).await?;
        if self.cancel_after_fence.load(Ordering::SeqCst) {
            assert!(layer2_core::agent_ownership::execution_authority().is_none());
            TransportAuthority::revoke(self.cancel_authority.as_ref());
        }
        Ok(())
    }
}
struct Resources;
#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        Err(SandboxResourceReadError::Unsupported(
            "local fixture".into(),
        ))
    }
}
struct Replica {
    coordinator: Arc<OwnershipCoordinator>,
    worker: Arc<AgentWorker>,
    address: String,
    entered: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Replica {
    fn drop(&mut self) {
        self.server.abort();
    }
}
struct Fixture {
    h: Harness<MongoSandboxStore>,
    admin: Client,
    name: String,
    app_name: String,
    ownership: Arc<MongoAgentOwnership>,
    replicas: Vec<Replica>,
    owner: usize,
    authority: Arc<Authority>,
    agent: Agent,
    deleting: bool,
    cancel_after_fence: Arc<AtomicBool>,
    failed_commands: Arc<AtomicUsize>,
    commands: Arc<Mutex<Vec<String>>>,
    usage: Arc<Mutex<measurements::Usage>>,
    ownership_io: Arc<AtomicUsize>,
    cancel_after_admission: Arc<AtomicBool>,
}
