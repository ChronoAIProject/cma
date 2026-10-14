use super::MemoryRuntime;
use crate::wire::WireOperation;
use crate::{AdapterError, AgentSlot};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ResourceKey {
    Worktree(String, String),
    AgentHome(AgentSlot),
    AgentSkill(AgentSlot, String),
    AgentProcess(AgentSlot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Disposition {
    Present,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
}

#[derive(Debug, Clone)]
pub(super) struct Authority {
    fence: u64,
    disposition: Disposition,
    identity: Identity,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn admit(
    runtime: &mut MemoryRuntime,
    resource: ResourceKey,
    disposition: Disposition,
    fence: u64,
    operation_id: &str,
    request_sha256: &str,
    payload_sha256: &str,
    operation: WireOperation,
) -> crate::Result<()> {
    if fence == 0 {
        return rejected(operation, "mutation_authority_invalid");
    }
    if let ResourceKey::AgentSkill(agent_id, _) = &resource {
        if runtime
            .authorities
            .get(&ResourceKey::AgentHome(agent_id.clone()))
            .is_some_and(|r| r.disposition == Disposition::Absent)
        {
            return rejected(operation, "mutation_authority_retired");
        }
    }
    let identity = Identity {
        operation_id: operation_id.into(),
        request_sha256: request_sha256.into(),
        payload_sha256: payload_sha256.into(),
    };
    if let Some(current) = runtime.authorities.get(&resource) {
        if current.disposition == Disposition::Absent && disposition == Disposition::Present {
            return rejected(operation, "mutation_authority_retired");
        }
        if fence < current.fence {
            return rejected(operation, "mutation_authority_stale");
        }
        if current.disposition == disposition && current.identity != identity {
            return rejected(operation, "mutation_authority_conflict");
        }
        if current.disposition == Disposition::Present
            && disposition == Disposition::Present
            && current.identity != identity
        {
            return rejected(operation, "mutation_authority_conflict");
        }
        if fence == current.fence && current.disposition == disposition {
            return Ok(());
        }
    }
    runtime.authorities.insert(
        resource,
        Authority {
            fence,
            disposition,
            identity,
        },
    );
    Ok(())
}

fn rejected<T>(operation: WireOperation, code: &str) -> crate::Result<T> {
    let detail = format!("{operation:?}: {code}");
    Err(
        if operation == WireOperation::OpenAgent && code == "mutation_authority_conflict" {
            AdapterError::OperationIdentityConflict(detail)
        } else {
            AdapterError::Rejected(detail)
        },
    )
}
