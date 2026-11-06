//! Explicit, bounded operator commands. Repair is read-only unless `--apply`
//! is supplied; target Agents are always named explicitly.

use std::{sync::Arc, time::SystemTime};

use layer2_core::AgentId;

use crate::{composition::ControlPlaneComposition, config::Config};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Serve,
    PrintRuntimeRole,
    RepairStaleTurns { agents: Vec<AgentId>, apply: bool },
}

pub(crate) fn parse(arguments: &[String]) -> Result<Command, String> {
    if arguments.is_empty() {
        return Ok(Command::Serve);
    }
    if arguments == ["--print-runtime-role"] {
        return Ok(Command::PrintRuntimeRole);
    }
    if arguments.first().map(String::as_str) != Some("repair-stale-turns") {
        return Err(usage());
    }
    let mut agents = Vec::new();
    let mut apply = false;
    let mut dry_run = false;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--agent" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    return Err("--agent requires an Agent id".into());
                };
                if !value.starts_with("agt_") || value.len() > 96 {
                    return Err("--agent must be a valid opaque Agent id".into());
                }
                agents.push(AgentId(value.clone()));
            }
            "--apply" => apply = true,
            "--dry-run" => dry_run = true,
            _ => return Err(usage()),
        }
        index += 1;
    }
    if agents.is_empty() || (apply && dry_run) || agents.len() > 64 {
        return Err(usage());
    }
    agents.sort();
    agents.dedup();
    Ok(Command::RepairStaleTurns { agents, apply })
}

pub(crate) async fn run_repair(agents: Vec<AgentId>, apply: bool) -> Result<(), String> {
    let config =
        Config::from_env().map_err(|error| format!("configuration is not usable: {error}"))?;
    if !apply {
        let client = mongodb::Client::with_uri_str(&config.mongodb_uri)
            .await
            .map_err(|error| format!("read-only Mongo connection failed: {error}"))?;
        let store = Arc::new(
            crate::store::MongoSandboxStore::open_read_only(&client, &config.mongodb_database)
                .await
                .map_err(|error| format!("read-only repair store failed: {error}"))?,
        );
        let adapter = crate::composition::repair_history_adapter(&config).await?;
        let report = crate::sandbox::inspect_stale_turns_read_only(store, adapter, &agents)
            .await
            .map_err(|error| format!("repair scan failed: {error}"))?;
        println!(
            "stale-turn repair mode=dry-run inspected={} eligible={} repaired=0 held={}",
            report.inspected, report.eligible, report.held
        );
        return Ok(());
    }
    let composition = ControlPlaneComposition::build(&config).await?;
    let report = composition
        .sandbox_lifecycle
        .repair_stale_turns(&agents, SystemTime::now(), apply)
        .await
        .map_err(|error| format!("repair scan failed: {error}"))?;
    println!(
        "stale-turn repair mode={} inspected={} eligible={} repaired={} held={}",
        if apply { "apply" } else { "dry-run" },
        report.inspected,
        report.eligible,
        report.repaired,
        report.held
    );
    Ok(())
}

fn usage() -> String {
    "usage: cma [--print-runtime-role] | repair-stale-turns --agent agt_... [--agent agt_...] [--dry-run|--apply]".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_defaults_to_dry_run_and_requires_explicit_targets() {
        let command = parse(&[
            "repair-stale-turns".into(),
            "--agent".into(),
            "agt_one".into(),
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::RepairStaleTurns {
                agents: vec![AgentId("agt_one".into())],
                apply: false
            }
        );
        assert!(parse(&["repair-stale-turns".into()]).is_err());
        assert!(parse(&[
            "repair-stale-turns".into(),
            "--agent".into(),
            "agt_one".into(),
            "--apply".into(),
            "--dry-run".into()
        ])
        .is_err());
    }
}
