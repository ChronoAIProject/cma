use super::{authority::Access, gate, writer_control, Result};

use crate::store::agent_log_segments::audit_command;
pub(super) use crate::store::agent_log_segments::audit_command::Action as AuditAction;

pub(super) enum Operation {
    Role(Access),
    Gate(gate::Command),
    ActiveProof(String),
    Audit {
        action: AuditAction,
        agent: String,
        id: String,
    },
}

impl Operation {
    pub(super) fn access(&self) -> Access {
        match self {
            Self::Role(access) => *access,
            Self::Gate(command) if matches!(command.action, writer_control::Action::Status) => {
                Access::GateRead
            }
            Self::Gate(_) => Access::GateWrite,
            Self::Audit { .. } => Access::Audit,
            Self::ActiveProof(_) => Access::ActiveProof,
        }
    }
}

pub(super) fn operation(arguments: &[String]) -> Result<Operation> {
    if gate::is_command(arguments) {
        return gate::parse(arguments).map(Operation::Gate);
    }
    if audit_command::is_command(arguments) {
        let command = audit_command::parse(arguments)?;
        return Ok(Operation::Audit {
            action: command.action,
            agent: command.agent_id,
            id: command.audit_id,
        });
    }
    let args: Vec<_> = arguments.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--agent-log-active-proof-prepare", agent]
            if !agent.is_empty()
                && agent.len() <= 512
                && !agent.starts_with('-')
                && *agent != "*"
                && !agent.chars().any(|c| c.is_control() || c.is_whitespace()) =>
        {
            Ok(Operation::ActiveProof((*agent).into()))
        }
        ["--print-agent-log-operator-role", kind] => Ok(Operation::Role(match *kind {
            "audit" => Access::Audit,
            "active-proof" => Access::ActiveProof,
            "gate-read" => Access::GateRead,
            "gate-write" => Access::GateWrite,
            _ => return Err("unknown operator role"),
        })),
        _ => Err("unsupported Agent log operator arguments"),
    }
}
