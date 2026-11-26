//! Construct the isolated MongoDB client for bounded reporting reads.
use crate::unavailable;
use layer2_core::sandbox_metrics::Result;
use mongodb::{options::ClientOptions, Client, Database};
use std::time::Duration;

pub const DB_DEADLINE: Duration = Duration::from_secs(2);

pub async fn connect(
    uri: &str,
    database: &str,
    pool_size: u32,
    configure_observation: impl FnOnce(&mut ClientOptions),
) -> Result<(Client, Database)> {
    let mut options = ClientOptions::parse(uri).await.map_err(unavailable)?;
    configure_pool(&mut options, pool_size);
    configure_observation(&mut options);
    let client = Client::with_options(options).map_err(unavailable)?;
    let database = client.database(database);
    Ok((client, database))
}

pub fn configure_pool(options: &mut ClientOptions, pool_size: u32) {
    options.max_pool_size = Some(pool_size);
    options.min_pool_size = Some(0);
    options.max_connecting = Some(1);
    options.connect_timeout = Some(DB_DEADLINE);
    options.server_selection_timeout = Some(DB_DEADLINE);
}
