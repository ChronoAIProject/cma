//! Owner-operated synthetic fixture for explicitly bound local-cluster cases.
mod bootstrap;
mod config;
#[path = "../support/replica_delivery.rs"]
mod delivery;
mod ledger;
#[cfg(test)]
mod ledger_tests;
mod oracle;
mod protocol;
#[cfg(test)]
mod protocol_tests;
mod reconcile;
mod request;
#[cfg(test)]
mod request_tests;
mod runner;
mod server;
#[path = "../relay/signer.rs"]
mod signer;

use std::{collections::BTreeMap, sync::Mutex, time::Duration};
struct Shared {
    config: config::Config,
    provider_bearer: String,
    channel_key: cmaeg_core::channel::Secret,
    channel_id: String,
    events: Mutex<BTreeMap<String, protocol::Event>>,
    ledger: Mutex<ledger::Ledger>,
    oracle: oracle::Oracle,
    run_lock: tokio::sync::Mutex<()>,
}
#[tokio::main]
async fn main() {
    if let Err(code) = execute().await {
        eprintln!("eg_fixture status=ERROR class={code}");
        std::process::exit(1);
    }
}
async fn execute() -> Result<(), &'static str> {
    let path = std::env::var_os("EG_DIAGNOSTIC_CONFIG").ok_or("config_path_missing")?;
    let config = config::Config::read(std::path::Path::new(&path))?;
    // Bind before migration so a port collision cannot leave a partially prepared case.
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|_| "fixture_listen")?;
    let shared = tokio::time::timeout(Duration::from_secs(60), bootstrap::prepare(config))
        .await
        .map_err(|_| "fixture_setup_deadline")??;
    eprintln!(
        "eg_fixture status=READY database_sha256={}",
        ledger::hash(shared.config.database.as_bytes())
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let serve =
        axum::serve(listener, server::router(shared.clone())).with_graceful_shutdown(async {
            let _ = stopped.await;
        });
    use std::future::IntoFuture;
    let serve = serve.into_future();
    tokio::pin!(serve);
    let result = tokio::select! {
        result = &mut serve => result.map_err(|_| "fixture_server"),
        _ = signal() => {
            let closed = shared.ledger.lock().map_err(|_| "fixture_ledger_lock")?.close();
            let _ = stop.send(());
            let result = tokio::time::timeout(Duration::from_secs(60), &mut serve).await;
            match result {
                Ok(Ok(())) => closed,
                Ok(Err(_)) => Err("fixture_server_shutdown"),
                Err(_) => Err("fixture_server_shutdown_deadline"),
            }
        }
    };
    let oracle_closed = tokio::time::timeout(Duration::from_secs(3), shared.oracle.shutdown())
        .await
        .is_ok();
    config::private_json(
        &shared.config.private_output.join("shutdown.json"),
        &serde_json::json!({
            "server_graceful":result.is_ok(),"oracle_client_shutdown":oracle_closed,
            "server_shutdown_budget_ms":60000,"oracle_shutdown_budget_ms":3000,
            "database_retained":true,"gateway_process_finality":"requires_outer_evidence"
        }),
    )?;
    let snapshot = shared
        .ledger
        .lock()
        .map_err(|_| "fixture_ledger_lock")?
        .snapshot();
    config::private_json(
        &shared.config.private_output.join("final-ledger.json"),
        &snapshot,
    )?;
    // The environment owner removes only the proven-owned database after pod
    // custody and final reconciliation. Server exit never silently drops it.
    eprintln!("eg_fixture status=STOPPED ledger_retained=true database_retained=true");
    result?;
    if !oracle_closed {
        return Err("fixture_oracle_shutdown_deadline");
    }
    Ok(())
}
async fn signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("fixture TERM handler");
        tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
