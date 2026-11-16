//! Account deletion acceptance and shared access revocation only.
use std::sync::Arc;

use crate::{auth::account_standing::ErasingSnapshot, config::Config, store::MongoStores};
use layer2_core::account_deletion::{AccountDeletionService, AccountStanding};

pub(super) async fn compose(
    config: &Config,
    stores: &MongoStores,
) -> Result<(Arc<AccountDeletionService>, Arc<dyn AccountStanding>), String> {
    let snapshot = Arc::new(ErasingSnapshot::new(stores.account_deletions.clone()));
    snapshot.start().await;
    let service = Arc::new(AccountDeletionService::new(
        stores.account_deletions.clone(),
        snapshot.clone(),
        config.account_deletion_enabled,
    ));
    Ok((service, snapshot))
}
