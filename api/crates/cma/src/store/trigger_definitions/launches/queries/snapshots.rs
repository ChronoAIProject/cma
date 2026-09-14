use super::*;

pub(in super::super) async fn snapshot_launch(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    session: &mut ClientSession,
) -> Result<Option<StoredLaunch>, LaunchStoreError> {
    store
        .launch_data
        .launches
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(read_launch)
        .transpose()
}

pub(in super::super) async fn snapshot_owned_launch(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<Option<Launch>, LaunchStoreError> {
    let document = store
        .launch_data
        .launches
        .find_one(doc! { "_id": id.as_str(), "owner": owner.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(unavailable)?;
    let Some(document) = document else {
        return Ok(None);
    };
    let launch = read_launch(&document)?.launch;
    if &launch.id != id || &launch.owner != owner {
        return Err(corrupt("owned Launch snapshot crossed its boundary"));
    }
    Ok(Some(launch))
}

pub(in super::super) async fn snapshot_handoff(
    store: &MongoTriggerDefinitionStore,
    id: &ServiceHandoffId,
    session: &mut ClientSession,
) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
    store
        .launch_data
        .handoffs
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(read_handoff)
        .transpose()
}
