use futures_util::future::BoxFuture;
use layer3_adapter::{resource_execution_protocol::*, ResourceInput};

use super::gate::EnterPermit;

/// Assigned and persisted before prepare. Implementations bind this identity to
/// retained kernel handles; neither a PID nor an Agent-wide process is a cell.
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Cell {
    pub execution: ExecutionKey,
    pub nonce: OpaqueId,
}

#[derive(Clone, Copy)]
pub(super) enum Termination {
    Cooperative,
    Force,
}

pub(super) struct Quiescent {
    pub evidence: StopEvidenceKind,
    pub state: QuiescenceV1,
}

pub(super) trait SharedExecutionRunner: Send + Sync {
    fn capabilities(&self) -> Option<ExecutionCapabilities> {
        None
    }
    fn prepare<'a>(
        &'a self,
        cell: &'a Cell,
        input: ResourceInput,
    ) -> BoxFuture<'a, ProtocolResult<()>>;
    /// Release only the sealed bootstrap, synchronously and without blocking IO.
    fn enter(&self, cell: &Cell, permit: EnterPermit<'_>) -> ProtocolResult<()>;
    /// Independent control; must not wait for a data sink or an ordinary queue.
    fn cancel(&self, cell: &Cell, termination: Termination) -> ProtocolResult<()>;
    fn observe_quiescence<'a>(&'a self, cell: &'a Cell)
        -> BoxFuture<'a, ProtocolResult<Quiescent>>;
    fn recover<'a>(&'a self, cell: &'a Cell) -> BoxFuture<'a, ProtocolResult<()>>;
}

pub(super) struct UnavailableRunner;
impl SharedExecutionRunner for UnavailableRunner {
    fn prepare<'a>(&'a self, _: &'a Cell, _: ResourceInput) -> BoxFuture<'a, ProtocolResult<()>> {
        Box::pin(async { Err(ProtocolError::Unsupported) })
    }
    fn enter(&self, _: &Cell, _: EnterPermit<'_>) -> ProtocolResult<()> {
        Err(ProtocolError::Unsupported)
    }
    fn cancel(&self, _: &Cell, _: Termination) -> ProtocolResult<()> {
        Err(ProtocolError::Unsupported)
    }
    fn observe_quiescence<'a>(&'a self, _: &'a Cell) -> BoxFuture<'a, ProtocolResult<Quiescent>> {
        Box::pin(async { Err(ProtocolError::Unknown) })
    }
    fn recover<'a>(&'a self, _: &'a Cell) -> BoxFuture<'a, ProtocolResult<()>> {
        Box::pin(async { Err(ProtocolError::Unknown) })
    }
}
