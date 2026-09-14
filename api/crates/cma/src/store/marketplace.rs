//! MongoDB implementation of the trusted profile marketplace port.

use layer2_core::ids::SubjectId;
use layer2_core::profiles::store::{ProfileStoreError, ProfileUseRecorder};
use layer2_core::profiles::{
    ForkedProfile, MarketplaceEntry, MarketplaceError, MarketplaceLocator, MarketplacePage,
    MarketplaceQuery, MarketplaceStore, ProfileCapacityBounds, ProfileVisibility,
};

use super::sandbox_mongo::MongoProfileStore;

#[async_trait::async_trait]
impl MarketplaceStore for MongoProfileStore {
    async fn list_marketplace(
        &self,
        query: &MarketplaceQuery,
    ) -> Result<MarketplacePage, MarketplaceError> {
        super::marketplace_query::list_marketplace(self, query).await
    }

    async fn read_marketplace(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<MarketplaceEntry, MarketplaceError> {
        super::marketplace_query::read_marketplace(self, locator).await
    }

    async fn set_profile_visibility(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        visibility: ProfileVisibility,
        updated_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn layer2_core::profiles::PublicValueScanner,
    ) -> Result<(), MarketplaceError> {
        super::marketplace_visibility::set_profile_visibility(
            self, owner, locator, visibility, updated_at, bounds, scanner,
        )
        .await
    }

    async fn fork_profile(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        revision: u32,
        now: String,
    ) -> Result<ForkedProfile, MarketplaceError> {
        super::marketplace_mutations::fork_profile(self, owner, locator, revision, now).await
    }
}

#[async_trait::async_trait]
impl ProfileUseRecorder for MongoProfileStore {
    async fn record_profile_use(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<(), ProfileStoreError> {
        super::marketplace_mutations::record_profile_use(self, locator).await
    }
}
