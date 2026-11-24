//! Preserve configured driver defaults and attach startup command observations.
use layer2_core::StoreError;
use mongodb::options::ClientOptions;

pub(super) async fn connection_options(
    uri: &str,
    database: &str,
) -> Result<ClientOptions, StoreError> {
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
    super::super::transaction_barrier::bind(&mut options, database);
    // Replica ownership changes authority, not the deployment's connection
    // budget. Preserve explicit URI options and driver defaults in both modes.
    super::super::mongo_latency::observe(&mut options);
    #[cfg(test)]
    if let Ok(handler) = STARTUP_COMMANDS.try_with(Clone::clone) {
        options.command_event_handler = Some(handler);
    }
    Ok(options)
}

#[cfg(test)]
tokio::task_local! {
    pub(in crate::store) static STARTUP_COMMANDS: mongodb::event::EventHandler<mongodb::event::command::CommandEvent>;
}
