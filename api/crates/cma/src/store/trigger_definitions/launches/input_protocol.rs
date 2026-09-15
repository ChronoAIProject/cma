//! Immutable admission receipt survives accidental removal of the aggregate sidecar.
use layer2_core::trigger_domain::{Launch, LaunchStoreError};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use super::queries::ReadError;
use super::{corrupt, unavailable, MongoTriggerDefinitionStore};
use crate::store::sandbox_mongo::record_bson;
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

pub(super) fn document(launch: &Launch) -> Result<Option<Document>, LaunchStoreError> {
    let Some(protocol) = &launch.input_protocol else {
        return Ok(None);
    };
    protocol.require_launch(launch)?;
    Ok(Some(doc! {
        "_id": launch.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": launch.owner.as_str(),
        "public_id": launch.public_id.as_str(),
        "command_key_hash": &launch.command_key_hash,
        "record": record_bson(protocol).map_err(corrupt)?,
    }))
}

pub(super) async fn insert(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    if let Some(document) = document(launch).map_err(ReadError::Domain)? {
        store
            .launch_data
            .input_protocols
            .insert_one(document)
            .session(session)
            .await
            .map_err(ReadError::Driver)?;
    }
    Ok(())
}

pub(super) async fn verify_in_session(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    let actual = store
        .launch_data
        .input_protocols
        .find_one(doc! {"_id": launch.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ReadError::Driver)?;
    require_document(launch, actual).map_err(ReadError::Domain)?;
    let binding = bindings(store)
        .find_one(doc! {"_id":launch.id.as_str()})
        .session(session)
        .await
        .map_err(ReadError::Driver)?;
    require_binding(launch, binding.as_ref()).map_err(ReadError::Domain)
}

pub(super) async fn verify_snapshot(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<(), LaunchStoreError> {
    let actual = store
        .launch_data
        .input_protocols
        .find_one(doc! {"_id": launch.id.as_str()})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    require_document(launch, actual)?;
    let binding = bindings(store)
        .find_one(doc! {"_id":launch.id.as_str()})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(unavailable)?;
    require_binding(launch, binding.as_ref())
}

pub(in crate::store) fn require_document(
    launch: &Launch,
    actual: Option<Document>,
) -> Result<(), LaunchStoreError> {
    if actual != document(launch)? {
        return Err(corrupt(
            "Launch input protocol and immutable admission receipt disagree",
        ));
    }
    Ok(())
}

fn bindings(store: &MongoTriggerDefinitionStore) -> mongodb::Collection<Document> {
    crate::store::durability::majority_collection(
        &store
            .client
            .database(&store.launch_data.input_protocols.namespace().db),
        crate::store::trigger_first_messages::COLLECTION,
    )
}
fn require_binding(launch: &Launch, actual: Option<&Document>) -> Result<(), LaunchStoreError> {
    crate::store::trigger_first_messages::require_launch_binding(launch, actual)
        .map_err(|error| corrupt(error.to_string()))
}
