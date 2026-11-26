use std::path::Path;

use super::{corrupt, validate_terminal_shape, Operation, Record, Subject};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use layer3_adapter::AgentProcessAction;

pub(super) fn validate_record(record: &Record, config: &RuntimeConfig) -> Result<()> {
    if record.version != 1 {
        return Err(corrupt("runtime mutation journal version is unsupported"));
    }
    if record.runtime_generation != crate::mutation_scope::generation(config, &record.subject) {
        return Err(corrupt(
            "runtime mutation journal belongs to another process generation",
        ));
    }
    validate_operation(&record.operation(), config)
        .map_err(|_| corrupt("runtime mutation operation identity is invalid"))?;
    if record
        .receipt
        .as_ref()
        .is_some_and(|receipt| !valid_receipt(record, receipt, config))
    {
        return Err(corrupt("runtime mutation receipt is invalid"));
    }
    validate_terminal_shape(record)
}

pub(super) fn validate_operation(operation: &Operation, config: &RuntimeConfig) -> Result<()> {
    let valid = operation.operation_id.len() <= 256
        && lower_sha256(&operation.request_sha256)
        && lower_sha256(&operation.payload_sha256)
        && match &operation.subject {
            Subject::AddWorktree {
                workspace,
                name,
                base,
            } => {
                operation.operation_id == format!("worktree:{workspace}:{name}:add:v1")
                    && operation.payload_sha256 == operation.request_sha256
                    && crate::paths::safe_component(workspace, "workspace name").is_ok()
                    && crate::paths::safe_component(name, "worktree name").is_ok()
                    && crate::paths::validate_selector(base, "worktree base").is_ok()
                    && layer3_adapter::WorktreeRequest {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        workspace: workspace.clone(),
                        name: name.clone(),
                        base: base.clone(),
                    }
                    .has_valid_sha256()
            }
            Subject::RemoveWorktree { workspace, name } => {
                operation.operation_id == format!("worktree:{workspace}:{name}:remove:v1")
                    && operation.payload_sha256 == operation.request_sha256
                    && crate::paths::safe_component(workspace, "workspace name").is_ok()
                    && crate::paths::safe_component(name, "worktree name").is_ok()
                    && layer3_adapter::WorktreeRef {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        workspace: workspace.clone(),
                        name: name.clone(),
                    }
                    .has_valid_sha256()
            }
            Subject::InstallAgentSkill { request } => {
                crate::agent_skill_install::validate(request).is_ok()
                    && request.operation_id == operation.operation_id
                    && request.request_sha256 == operation.request_sha256
                    && operation.payload_sha256 == operation.request_sha256
            }
            Subject::CreateAgentHome { agent_id } => {
                valid_agent_identity(operation, agent_id, "home:create")
            }
            Subject::OpenAgent {
                agent_id,
                cwd,
                home,
            } => {
                valid_process_identity(operation, agent_id, AgentProcessAction::Open)
                    && Path::new(home) == config.agent_home(agent_id)
                    && crate::worktree::parse_path(&config.home, Path::new(cwd)).is_some()
            }
            Subject::CloseAgent { agent_id } => {
                valid_process_identity(operation, agent_id, AgentProcessAction::Close)
                    && operation.payload_sha256 == operation.request_sha256
                    && layer3_adapter::AgentCloseRequest {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        agent_id: layer3_adapter::AgentSlot::new(agent_id),
                    }
                    .has_valid_sha256()
            }
            Subject::RemoveAgentHome { agent_id } => {
                valid_agent_identity(operation, agent_id, "home:remove")
                    && operation.payload_sha256 == operation.request_sha256
                    && layer3_adapter::AgentHomeRemoveRequest {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        agent_id: layer3_adapter::AgentSlot::new(agent_id),
                    }
                    .has_valid_sha256()
            }
        };
    valid
        .then_some(())
        .ok_or_else(|| corrupt("runtime mutation operation identity is invalid"))
}

fn valid_agent_identity(operation: &Operation, agent_id: &str, action: &str) -> bool {
    crate::paths::safe_component(agent_id, "agent id").is_ok()
        && operation.operation_id == format!("agent:{agent_id}:{action}:v1")
}

fn valid_process_identity(
    operation: &Operation,
    agent_id: &str,
    action: AgentProcessAction,
) -> bool {
    crate::paths::safe_component(agent_id, "agent id").is_ok()
        && action
            .incarnation(agent_id, &operation.operation_id)
            .is_some()
}

fn valid_receipt(record: &Record, value: &serde_json::Value, config: &RuntimeConfig) -> bool {
    match &record.subject {
        Subject::AddWorktree {
            workspace, name, ..
        } => serde_json::from_value::<layer3_adapter::WorktreeReceipt>(value.clone()).is_ok_and(
            |receipt| {
                receipt_identity(record, &receipt.operation_id, &receipt.request_sha256)
                    && receipt.workspace == *workspace
                    && receipt.name == *name
                    && Path::new(&receipt.path)
                        == crate::worktree::expected_path(&config.home, workspace, name)
                    && receipt.branch == format!("cma/{name}")
                    && valid_commit(&receipt.base_commit)
            },
        ),
        Subject::RemoveWorktree { workspace, name } => {
            serde_json::from_value::<layer3_adapter::WorktreeRemoveReceipt>(value.clone())
                .is_ok_and(|receipt| {
                    receipt_identity(record, &receipt.operation_id, &receipt.request_sha256)
                        && receipt.workspace == *workspace
                        && receipt.name == *name
                })
        }
        Subject::InstallAgentSkill { request } => {
            serde_json::from_value::<layer3_adapter::AgentSkillInstallReceipt>(value.clone())
                .is_ok_and(|receipt| receipt.matches(request))
        }
        Subject::CreateAgentHome { agent_id } => {
            serde_json::from_value::<layer3_adapter::wire::AgentHomeReceipt>(value.clone())
                .is_ok_and(|receipt| {
                    receipt_identity(record, &receipt.operation_id, &receipt.request_sha256)
                        && receipt.agent_id.as_str() == agent_id
                        && Path::new(&receipt.path) == config.agent_home(agent_id)
                })
        }
        Subject::OpenAgent {
            agent_id,
            cwd,
            home,
        } => serde_json::from_value::<layer3_adapter::wire::AgentOpenReceipt>(value.clone())
            .is_ok_and(|receipt| {
                receipt_identity(record, &receipt.operation_id, &receipt.request_sha256)
                    && receipt.agent_id.as_str() == agent_id
                    && receipt.cwd == *cwd
                    && receipt.home == *home
                    && receipt.conversation_id.is_none()
            }),
        Subject::CloseAgent { agent_id } => {
            serde_json::from_value::<layer3_adapter::wire::AgentCloseReceipt>(value.clone())
                .is_ok_and(|receipt| {
                    receipt_identity(record, &receipt.operation_id, &receipt.request_sha256)
                        && receipt.agent_id.as_str() == agent_id
                })
        }
        Subject::RemoveAgentHome { agent_id } => {
            serde_json::from_value::<layer3_adapter::wire::AgentHomeRemoveReceipt>(value.clone())
                .is_ok_and(|receipt| {
                    receipt_identity(record, &receipt.operation_id, &receipt.request_sha256)
                        && receipt.agent_id.as_str() == agent_id
                })
        }
    }
}

fn receipt_identity(record: &Record, operation_id: &str, request_sha256: &str) -> bool {
    operation_id == record.operation_id && request_sha256 == record.request_sha256
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(lower_hex)
}

fn lower_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(lower_hex)
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}
