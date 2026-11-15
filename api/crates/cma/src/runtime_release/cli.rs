//! Explicit private-file interface. No control command launches serving/workers.
use super::{files, verifier::LiveVerifier};
use crate::{config::Config, release_authority::*};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Status {
        deployment: String,
    },
    Operation {
        deployment: String,
        id: String,
    },
    Effect {
        deployment: String,
        id: String,
    },
    MigrationPlan,
    Bootstrap {
        request: BootstrapRequest,
        cookie_file: PathBuf,
        grant_file: PathBuf,
    },
    Begin {
        request: TransitionRequest,
        cookie_file: PathBuf,
        grant_file: PathBuf,
    },
    Advance {
        request: AdvanceRequest,
        cookie_file: PathBuf,
        grant_file: PathBuf,
    },
    Settle {
        permit: EffectPermit,
        operator: OperatorIdentity,
        cookie_file: PathBuf,
        grant_file: PathBuf,
    },
}

pub(super) fn is_command(args: &[String]) -> bool {
    args.first()
        .is_some_and(|value| value == "--runtime-release")
}

pub(super) async fn command(args: &[String]) -> std::result::Result<String, String> {
    if args.len() != 3 {
        return Err("usage: cma --runtime-release REQUEST_FILE RECEIPT_FILE".into());
    }
    let bytes = files::read_private(std::path::Path::new(&args[1]), 64 * 1024)
        .map_err(|_| "release request must be a bounded private regular file")?;
    let request: Command =
        serde_json::from_slice(&bytes).map_err(|_| "release command is malformed")?;
    if matches!(request, Command::MigrationPlan) {
        files::create_private(std::path::Path::new(&args[2]), &super::migration::plan())?;
        return Ok("runtime release migration plan retained; no apply authority".into());
    }
    let config = Config::from_env()?;
    let client = super::connect(&config).await?;
    let store = AuthorityStore::open(client.clone(), client.database(&config.mongodb_database))
        .await
        .map_err(|error| error.to_string())?;
    let output = match request {
        Command::Status { deployment } => {
            same_deployment(&config, &deployment)?;
            serde_json::to_value(
                store
                    .load(&deployment)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::Operation { deployment, id } => {
            same_deployment(&config, &deployment)?;
            serde_json::to_value(
                store
                    .operation(&deployment, &id)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::Effect { deployment, id } => {
            same_deployment(&config, &deployment)?;
            serde_json::to_value(
                store
                    .effect(&deployment, &id)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::Bootstrap {
            request,
            cookie_file,
            grant_file,
        } => {
            same_deployment(&config, &request.deployment)?;
            let verifier = LiveVerifier::open(
                &config,
                &cookie_file,
                &grant_file,
                &request.operator,
                &OperationRequest::Bootstrap(Box::new(request.clone())),
            )
            .await?;
            serde_json::to_value(
                store
                    .bootstrap(&request, &verifier)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::Begin {
            request,
            cookie_file,
            grant_file,
        } => {
            same_deployment(&config, &request.deployment)?;
            let verifier = LiveVerifier::open(
                &config,
                &cookie_file,
                &grant_file,
                &request.operator,
                &OperationRequest::Transition(Box::new(request.clone())),
            )
            .await?;
            serde_json::to_value(
                store
                    .begin(&request, &verifier)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::Advance {
            request,
            cookie_file,
            grant_file,
        } => {
            same_deployment(&config, &request.deployment)?;
            let verifier = LiveVerifier::open(
                &config,
                &cookie_file,
                &grant_file,
                &request.operator,
                &request,
            )
            .await?;
            serde_json::to_value(
                store
                    .advance(&request, &verifier)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::Settle {
            permit,
            operator,
            cookie_file,
            grant_file,
        } => {
            same_deployment(&config, &permit.writer.deployment)?;
            let verifier =
                LiveVerifier::open(&config, &cookie_file, &grant_file, &operator, &permit).await?;
            serde_json::to_value(
                store
                    .settle_effect(&permit, &verifier)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        Command::MigrationPlan => unreachable!("handled before database connection"),
    }
    .map_err(|_| "release receipt encoding failed")?;
    files::create_private(std::path::Path::new(&args[2]), &output)?;
    Ok("runtime release receipt retained; inspect exact operation outcome".into())
}

fn same_deployment(config: &Config, deployment: &str) -> std::result::Result<(), String> {
    if deployment == config.deployment_id {
        Ok(())
    } else {
        Err("foreign release deployment refused".into())
    }
}
