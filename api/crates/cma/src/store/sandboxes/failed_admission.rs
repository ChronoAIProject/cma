use super::*;
use layer2_core::sandbox::{SandboxEventKind, SandboxLifecycleAdmission, SandboxStatus};
use mongodb::options::ReadConcern;
use std::time::Duration;

pub(super) async fn read(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
) -> Result<Option<SandboxLifecycleAdmission>, SandboxStoreError> {
    let unavailable =
        |error: mongodb::error::Error| SandboxStoreError::Unavailable(error.to_string());
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let current = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .max_time(Duration::from_secs(2))
        .await
        .map_err(unavailable)?;
    if current.as_ref().map(read_sandbox).transpose()?.as_ref() != Some(sandbox) {
        return Ok(None);
    }
    let mut cursor = store
        .sandbox_events
        .find(doc! {
            "sandbox_id": sandbox.id.as_str(), "record.kind.type": { "$in": ["status_changed", "runtime_recovery_admitted", "revived", "rotated"] },
        })
        .sort(doc! { "seq": -1 })
        .limit(3)
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .max_time(Duration::from_secs(2))
        .await
        .map_err(unavailable)?;
    let mut transitions = Vec::new();
    while let Some(document) = cursor
        .next(&mut session)
        .await
        .transpose()
        .map_err(unavailable)?
    {
        transitions.push(super::super::sandbox_events::read_event(&document)?);
    }
    let Some(event) = SandboxLifecycleAdmission::event(sandbox, &transitions) else {
        return Ok(None);
    };
    let SandboxEventKind::StatusChanged { to, .. } = event.kind else {
        return Ok(None);
    };
    let mut controls = Vec::new();
    if let Some(command) = match to {
        SandboxStatus::Pausing => Some("pause"),
        SandboxStatus::Reviving => Some("revive"),
        _ => None,
    } {
        let mut cursor = store
            .sandbox_controls
            .find(doc! {
                "sandbox_id": sandbox.id.as_str(), "queued": false,
                "record.envelope.command": command, "record.status.status": "applied",
                "record.status.disposition": "scheduled", "record.status.at": &event.appended_at,
            })
            .limit(2)
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .max_time(Duration::from_secs(2))
            .await
            .map_err(unavailable)?;
        while let Some(document) = cursor
            .next(&mut session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            controls.push(
                super::super::sandbox_controls::records::read(&document)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
            );
        }
    }
    Ok(SandboxLifecycleAdmission::from_history(
        sandbox,
        &transitions,
        &controls,
    ))
}
