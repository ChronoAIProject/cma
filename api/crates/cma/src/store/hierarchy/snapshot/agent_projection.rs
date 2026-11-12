//! Shared query machinery selects full records or a bounded lineage projection.
use super::*;
use layer2_core::sandbox::Agent;

pub(super) trait AgentReader {
    type Record;
    fn projection() -> Option<Document>;
    fn read(document: &Document) -> Result<Self::Record, SandboxStoreError>;
    fn id(record: &Self::Record) -> &str;
}

pub(super) struct Complete;
impl AgentReader for Complete {
    type Record = Agent;
    fn projection() -> Option<Document> {
        None
    }
    fn read(document: &Document) -> Result<Agent, SandboxStoreError> {
        read_agent(document)
    }
    fn id(record: &Agent) -> &str {
        record.id.as_str()
    }
}
