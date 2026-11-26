use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::inventory::{Inventory, Kind};
use super::super::{invalid, Budget};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::{Disposition, Record as Authority};
use crate::mutation_journal::{Operation, Record as Journal, Status};

#[derive(Serialize)]
pub(super) struct Record {
    source_index: usize,
    source_sha256: String,
    source_bytes: u64,
    scope: Scope,
    kind: Kind,
    unpublished: bool,
    fact: Option<Fact>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Scope {
    LegacyGlobal,
    ProcessGeneration { generation: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Fact {
    Mutation {
        operation: OperationFact,
        status: Status,
    },
    Authority {
        operation: OperationFact,
        resource_sha256: String,
        fence: String,
        disposition: Disposition,
    },
}

#[derive(Serialize)]
struct OperationFact {
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
    subject_sha256: String,
}

impl OperationFact {
    fn new(operation: Operation) -> Result<Self> {
        Ok(Self {
            subject_sha256: digest(&operation.subject)?,
            operation_id: operation.operation_id,
            request_sha256: operation.request_sha256,
            payload_sha256: operation.payload_sha256,
        })
    }
}

fn digest(value: &impl Serialize) -> Result<String> {
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = HashWriter(Sha256::new());
    serde_json::to_writer(&mut hash, value).map_err(|_| invalid())?;
    Ok(crate::command::hex(&hash.0.finalize()))
}

pub(super) fn project(
    config: &RuntimeConfig,
    inventory: &Inventory,
    index: usize,
    budget: &mut Budget,
) -> Result<Record> {
    let entry = &inventory.entries[index];
    let root = &inventory.roots[entry.directory];
    let bytes = inventory.read(index, budget)?;
    let mut scoped = config.clone();
    if root.scope != 0 {
        scoped.generation = root.scope;
    }
    let fact = match root.kind {
        Kind::Authority if !entry.temporary => {
            let record =
                Authority::decode_inventory(&bytes, &scoped, &root.path.join(&entry.name))?;
            Some(Fact::Authority {
                operation: OperationFact::new(record.operation())?,
                resource_sha256: digest(&record.resource)?,
                fence: record.authority_fence.to_string(),
                disposition: record.disposition,
            })
        }
        Kind::Journal => {
            let record = Journal::decode(&bytes, &scoped)?;
            Some(Fact::Mutation {
                operation: OperationFact::new(record.operation())?,
                status: record.status(),
            })
        }
        _ => None,
    };
    Ok(Record {
        source_index: index,
        source_sha256: entry.sha256.clone(),
        source_bytes: entry.identity.bytes,
        scope: if root.scope == 0 {
            Scope::LegacyGlobal
        } else {
            Scope::ProcessGeneration {
                generation: root.scope.to_string(),
            }
        },
        kind: root.kind,
        unpublished: entry.temporary,
        fact,
    })
}
