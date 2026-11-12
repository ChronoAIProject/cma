//! Parse operator control arguments. The shared CLI verifies authority, schema
//! and topology before invoking the returned action. Never run from startup.
use super::{
    writer_control::Action,
    writer_gate::{GateName, Policy},
};
use std::{fs::File, io::Read};

pub(in crate::store) struct Command {
    pub name: GateName,
    pub action: Action,
}

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments.first().is_some_and(|arg| {
        matches!(
            arg.as_str(),
            "--agent-log-gate-status" | "--agent-log-gate-replace" | "--agent-log-gate-close"
        )
    })
}

pub(in crate::store) fn parse(arguments: &[String]) -> Result<Command, &'static str> {
    let args = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let (gate, action) = match args.as_slice() {
        ["--agent-log-gate-status", gate] => (*gate, Action::Status),
        ["--agent-log-gate-close", gate, revision] => (*gate, Action::Close {expected_revision: revision_number(revision)?}),
        ["--agent-log-gate-replace", gate, revision, path] => {
            let expected_revision = revision_number(revision)?;
            let file = File::open(path).map_err(|_| "gate policy file unavailable")?;
            let mut json = String::new();
            file.take(48 * 1024 + 1).read_to_string(&mut json).map_err(|_| "gate policy is not UTF-8")?;
            let policy = Policy::parse(&json).map_err(|_| "invalid gate policy")?;
            (*gate, Action::Replace {expected_revision, policy})
        }
        _ => return Err("usage: --agent-log-gate-status NAME | --agent-log-gate-close NAME REVISION | --agent-log-gate-replace NAME REVISION POLICY_FILE"),
    };
    let name = match gate {
        "agent_log_segments_v1" => GateName::Segments,
        "agent_log_capacity_recovery_v1" => GateName::CapacityRecovery,
        _ => return Err("unknown Agent log gate"),
    };
    Ok(Command { name, action })
}

fn revision_number(value: &str) -> Result<i64, &'static str> {
    let revision = value.parse::<i64>().map_err(|_| "invalid gate revision")?;
    if revision < 0 || revision == i64::MAX || revision.to_string() != value {
        return Err("invalid gate revision");
    }
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_requires_exact_gate_and_explicit_cas_revision() {
        let args = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert!(parse(&args(&["--agent-log-gate-status", "agent_log_segments_v1"])).is_ok());
        assert!(parse(&args(&[
            "--agent-log-gate-close",
            "agent_log_capacity_recovery_v1",
            "0"
        ]))
        .is_ok());
        for bad in ["-1", "01", "+1", " 1", "9223372036854775807"] {
            assert!(parse(&args(&[
                "--agent-log-gate-close",
                "agent_log_segments_v1",
                bad
            ]))
            .is_err());
        }
        assert!(parse(&args(&["--agent-log-gate-close", "agent_log_segments_v1"])).is_err());
        assert!(parse(&args(&["--agent-log-gate-status", "unknown"])).is_err());
        assert!(parse(&args(&["--migrate-agent-log-segments"])).is_err());
    }
}
