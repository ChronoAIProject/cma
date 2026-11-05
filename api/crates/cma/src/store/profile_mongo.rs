//! MongoDB collections and initialization for profile drafts and revisions.

use layer2_core::StoreError;
use mongodb::{bson::Document, Client, Collection};

use super::{
    durability::majority_collection,
    sandbox_mongo::{
        AGENT_PROFILES, AGENT_PROFILE_REVISIONS, SANDBOX_PROFILES, SANDBOX_PROFILE_REVISIONS,
        WORKSPACE_PROFILES, WORKSPACE_PROFILE_REVISIONS,
    },
    schema::initialize_collections,
};

const PROFILE_COLLECTIONS: &[&str] = &[
    AGENT_PROFILES,
    AGENT_PROFILE_REVISIONS,
    WORKSPACE_PROFILES,
    WORKSPACE_PROFILE_REVISIONS,
    SANDBOX_PROFILES,
    SANDBOX_PROFILE_REVISIONS,
];

/// Profile drafts and revisions are separate from runtime aggregates, but use
/// the same Mongo client so publication can resolve references transactionally.
#[derive(Clone)]
pub struct MongoProfileStore {
    pub(super) client: Client,
    pub(super) agent_profiles: Collection<Document>,
    pub(super) agent_revisions: Collection<Document>,
    pub(super) workspace_profiles: Collection<Document>,
    pub(super) workspace_revisions: Collection<Document>,
    pub(super) sandbox_profiles: Collection<Document>,
    pub(super) sandbox_revisions: Collection<Document>,
}

impl MongoProfileStore {
    pub(crate) async fn open(client: &Client, name: &str) -> Result<Self, StoreError> {
        let database = client.database(name);
        initialize_collections(&database, PROFILE_COLLECTIONS).await?;
        let store = Self {
            client: client.clone(),
            agent_profiles: majority_collection(&database, AGENT_PROFILES),
            agent_revisions: majority_collection(&database, AGENT_PROFILE_REVISIONS),
            workspace_profiles: majority_collection(&database, WORKSPACE_PROFILES),
            workspace_revisions: majority_collection(&database, WORKSPACE_PROFILE_REVISIONS),
            sandbox_profiles: majority_collection(&database, SANDBOX_PROFILES),
            sandbox_revisions: majority_collection(&database, SANDBOX_PROFILE_REVISIONS),
        };
        super::profiles::ensure_indexes(&store).await?;
        tracing::info!(database = name, "sandbox-profile Mongo store ready");
        Ok(store)
    }
}
