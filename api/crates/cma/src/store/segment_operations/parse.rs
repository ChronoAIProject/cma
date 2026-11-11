use super::{authority::Access, gate, writer_control, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuditAction {
    Unit,
    Cancel,
    Install,
}

impl AuditAction {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Unit => "audit_unit",
            Self::Cancel => "audit_cancel",
            Self::Install => "audit_install",
        }
    }
}

pub(super) enum Operation {
    Role(Access),
    Gate(gate::Command),
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
        }
    }
}

pub(super) fn operation(arguments: &[String]) -> Result<Operation> {
    if gate::is_command(arguments) {
        return gate::parse(arguments).map(Operation::Gate);
    }
    let args: Vec<_> = arguments.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--print-agent-log-operator-role", kind] => Ok(Operation::Role(match *kind {
            "audit" => Access::Audit,
            "gate-read" => Access::GateRead,
            "gate-write" => Access::GateWrite,
            _ => return Err("unknown operator role"),
        })),
        [command, agent, id] => {
            let action = match *command {
                "--agent-log-audit-unit" => AuditAction::Unit,
                "--agent-log-audit-cancel" => AuditAction::Cancel,
                "--agent-log-audit-install" => AuditAction::Install,
                _ => return Err("unknown Agent log operator command"),
            };
            for value in [agent, id] {
                if value.is_empty()
                    || value.len() > 128
                    || value.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err("operator identity is empty, oversized or malformed");
                }
            }
            Ok(Operation::Audit {
                action,
                agent: (*agent).into(),
                id: (*id).into(),
            })
        }
        _ => Err("unsupported Agent log operator arguments"),
    }
}
