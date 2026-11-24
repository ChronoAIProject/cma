mod config;
mod fixture;
mod ledger;
mod model;
mod monitor;
mod protocol;
mod runner;

use std::{path::Path, sync::Arc};

use config::Config;
use ledger::Ledger;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter("cma_journey_probe=info")
        .with_writer(std::io::stderr)
        .init();
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(reason = %error, "journey runner could not complete command");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), &'static str> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [command, config, rest @ ..] = args.as_slice() else {
        return Err("usage: cma-journey-probe <serve|init|export> CONFIG [FROM_UNIX UNTIL_UNIX] or fixture LOOPBACK_ADDRESS");
    };
    if command == "fixture" && rest.is_empty() {
        let address = config
            .to_str()
            .and_then(|v| v.parse().ok())
            .ok_or("invalid scripted provider address")?;
        return fixture::serve(address).await;
    }
    let config = Arc::new(Config::load(Path::new(config))?);
    let ledger = Ledger::new(&config);
    match (command.to_str(), rest) {
        (Some("init"), []) => ledger.init().map_err(|_| "cannot initialize journey state"),
        (Some("export"), [from, until]) => {
            let timestamp = |value: &std::ffi::OsStr| {
                value
                    .to_str()
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or("invalid Unix timestamp")
            };
            ledger
                .export(timestamp(from)?, timestamp(until)?, &config)
                .map_err(|_| "cannot export journey evidence")
        }
        (Some("serve"), []) => runner::serve(config, ledger).await,
        _ => Err("invalid journey runner command"),
    }
}

#[cfg(test)]
mod tests;
