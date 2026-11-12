//! Pure argument parsing. The shared operator dispatcher owns credentials,
//! authority preflight, deadlines and execution; parsing performs no I/O.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::store) enum Action {
    RunUnit,
    Cancel,
    Install,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::store) struct Command {
    pub action: Action,
    pub agent_id: String,
    pub audit_id: String,
}

pub(in crate::store) fn is_command(arguments: &[String]) -> bool {
    arguments
        .first()
        .is_some_and(|arg| arg.starts_with("--agent-log-audit-"))
}

pub(in crate::store) fn parse(arguments: &[String]) -> Result<Command, &'static str> {
    let [action, agent_id, audit_id] = arguments else {
        return Err("expected one audit action, Agent identifier and audit identifier");
    };
    let action = match action.as_str() {
        "--agent-log-audit-unit" => Action::RunUnit,
        "--agent-log-audit-cancel" => Action::Cancel,
        "--agent-log-audit-install" => Action::Install,
        _ => return Err("unsupported Agent log audit action"),
    };
    for (value, maximum) in [(agent_id, 512), (audit_id, 128)] {
        if value.is_empty()
            || value.len() > maximum
            || value.starts_with('-')
            || value == "*"
            || value.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("invalid Agent log audit identifier");
        }
    }
    Ok(Command {
        action,
        agent_id: agent_id.clone(),
        audit_id: audit_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn exact_actions_preserve_one_agent_and_audit_identity() {
        for (flag, action) in [
            ("--agent-log-audit-unit", Action::RunUnit),
            ("--agent-log-audit-cancel", Action::Cancel),
            ("--agent-log-audit-install", Action::Install),
        ] {
            let arguments = args(&[flag, "agt_fixture", "audit-fixture"]);
            assert!(is_command(&arguments));
            assert_eq!(
                parse(&arguments).unwrap(),
                Command {
                    action,
                    agent_id: "agt_fixture".into(),
                    audit_id: "audit-fixture".into(),
                }
            );
        }
    }

    #[test]
    fn malformed_actions_and_unbounded_identifiers_are_refused() {
        for values in [
            vec![],
            vec!["--agent-log-audit-unit"],
            vec!["--agent-log-audit-unit", "agt_fixture", "audit", "extra"],
            vec!["--agent-log-audit-reset", "agt_fixture", "audit"],
            vec!["--agent-log-audit-install", "*", "audit"],
            vec!["--agent-log-audit-unit", "agt_fixture", ""],
            vec!["--agent-log-audit-unit", "agt_fixture", "audit\nnext"],
            vec!["--agent-log-audit-unit", "agt_fixture", "--all"],
            vec!["--agent-log-audit-unit", "agt fixture", "audit"],
        ] {
            assert!(parse(&args(&values)).is_err());
        }
        for (agent, audit) in [
            ("a".repeat(513), "audit".into()),
            ("agt_fixture".into(), "b".repeat(129)),
        ] {
            assert!(parse(&args(&["--agent-log-audit-unit", &agent, &audit])).is_err());
        }
        assert!(!is_command(&[]));
        assert!(!is_command(&args(&["--migration-plan"])));
        assert!(is_command(&args(&["--agent-log-audit-unknown"])));
    }
}
