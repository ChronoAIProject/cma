use super::ACTIONS;
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    Database,
};

/// Required only by the enabled read service; installation remains additive.
pub(crate) async fn verify_observation(database: &Database) -> Result<(), StoreError> {
    let status = database
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(|_| StoreError::Unavailable("cannot verify observation grants".into()))?;
    validate_observation_grants(&status, database.name())
}
pub(super) fn validate_observation_grants(
    status: &Document,
    database: &str,
) -> Result<(), StoreError> {
    let role = doc! {"privileges": [{"resource": {"db": database, "collection": crate::store::agent_observation::COLLECTION}, "actions": ACTIONS}]};
    cma_service_auth::mongodb_authority::verify_shared_schema(status, database, &role)
        .map_err(|error| StoreError::Unavailable(error.into()))
}
