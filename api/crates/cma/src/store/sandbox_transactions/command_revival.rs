use layer2_core::sandbox::agent_io::CommandRevivalAuthorization;
use layer2_core::sandbox::store::{validate_command_revival_transition, SandboxStore};
use layer2_core::sandbox::Sandbox;
use mongodb::ClientSession;

use super::*;
use crate::store::{agent_commands, resource_access};

const MARKER: &str = "last_command_revival";

pub(super) struct Receipt {
    command_id: layer2_core::ids::AgentCommandId,
    marker: String,
}

pub(super) async fn fence(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    revival: (
        Option<&CommandRevivalAuthorization>,
        Option<&layer3_adapter::ReleasedRuntime>,
    ),
    candidate: &Sandbox,
    at: &str,
    event: &SandboxEventKind,
) -> Result<Option<Receipt>, ApplyError> {
    let (proof, release) = revival;
    let Some(proof) = proof else { return Ok(None) };
    proof.require_current().map_err(|_| conflict())?;
    store
        .bindings
        .conversation_authority(proof.binding_authority(), session)
        .await?;
    let snapshot = resource_access::read_execution_snapshot(store, &proof.target(), session)
        .await
        .map_err(|error| match error {
            resource_access::ReadError::Driver(error) => ApplyError::Driver(error),
            resource_access::ReadError::Domain(_) => conflict(),
        })?;
    proof.validate_snapshot(&snapshot).map_err(|_| conflict())?;
    let current = &snapshot.lineage.sandbox;
    let target = release.and_then(|release| store.managed_runtime_target(current, &release.agent));
    if let Some(release) = release.filter(|_| target.is_some()) {
        if current.has_managed_runtime_image() {
            store
                .fence_released_admission(
                    release,
                    layer2_core::released_runtime::ReleasedLane::Agent,
                    session,
                )
                .await
                .map_err(ApplyError::Domain)?;
        }
    }
    validate_command_revival_transition(&snapshot.lineage.sandbox, candidate, at, event, target)
        .map_err(ApplyError::Domain)?;
    let command_id = proof.command().receipt.envelope.command_id.clone();
    let document = store
        .agent_commands
        .find_one(doc! { "_id": command_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(conflict)?;
    let command = agent_commands::read_command(&document)
        .map_err(|error| ApplyError::Domain(agent_commands::binding::domain_error(error)))?;
    proof.require_command(&command).map_err(|_| conflict())?;
    let resources = proof.authorization_audits().map_err(|_| conflict())?;
    let audit = layer2_core::sandbox::agent_io::CommandAuthorizationAudit::new(
        &command,
        layer2_core::sandbox::agent_io::CommandAuthorizationEffect::Revival {
            sandbox_revision: candidate.revision,
        },
        resources,
    )
    .map_err(|error| ApplyError::Domain(agent_commands::binding::domain_error(error)))?;
    agent_commands::audit::append(store, &audit, session)
        .await
        .map_err(|error| match error {
            agent_commands::ApplyError::Domain(error) => {
                ApplyError::Domain(agent_commands::binding::domain_error(error))
            }
            agent_commands::ApplyError::Driver(error) => ApplyError::Driver(error),
        })?;
    // A real write serializes with command claims/refusals. Repeated identical
    // $set values can be no-ops, so every transaction attempt gets a fresh nonce.
    // Receipt/schema fields stay byte-for-byte unchanged.
    let receipt = Receipt {
        command_id,
        marker: uuid::Uuid::new_v4().to_string(),
    };
    let result = store.agent_commands.update_one(
        doc! { "_id": receipt.command_id.as_str(), "revision": signed(command.revision, "command revision").map_err(|_| conflict())? },
        doc! { "$set": { MARKER: &receipt.marker } },
    ).session(session).await.map_err(ApplyError::Driver)?;
    if result.matched_count != 1 || result.modified_count != 1 {
        return Err(conflict());
    }
    Ok(Some(receipt))
}

pub(super) fn require_current(transaction: &SandboxTransaction) -> Result<(), SandboxStoreError> {
    if let SandboxTransaction::SaveSandboxAndAppend {
        command_revival: Some(proof),
        ..
    } = transaction
    {
        proof
            .require_current()
            .map_err(|_| SandboxStoreError::Conflict)?;
    }
    Ok(())
}

pub(super) async fn matches(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    receipt: Option<&Receipt>,
) -> Result<bool, SandboxStoreError> {
    let Some(receipt) = receipt else {
        return Ok(true);
    };
    // Reconciliation observes committed evidence even after authority expires.
    // It cannot use a different command's identical parent/event as our receipt.
    let found = store
        .agent_commands
        .find_one(doc! { "_id": receipt.command_id.as_str(), MARKER: &receipt.marker })
        .projection(doc! { "_id": 1 })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(|_| {
            SandboxStoreError::Unavailable("command revival reconciliation is unavailable".into())
        })?;
    Ok(found.is_some())
}

fn conflict() -> ApplyError {
    ApplyError::Domain(SandboxStoreError::Conflict)
}
