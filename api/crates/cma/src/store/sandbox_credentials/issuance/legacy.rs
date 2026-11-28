//! Promotion cannot turn a legacy, possibly dispatched mint into fresh work.
use super::*;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::{
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRevivePhase, SandboxWorkStep,
};

pub(in crate::store) async fn preserve(
    store: &MongoSandboxStore,
    previous: &Sandbox,
    next: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), mongodb::error::Error> {
    if previous.work.authority_protocol.is_some()
        || next.work.authority_protocol.is_none()
        || (previous.work.lease.is_none() && previous.work.attempts == 0)
        || !may_have_minted(previous)
    {
        return Ok(());
    }
    let rows = store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection::<Document>(COLLECTION);
    if rows
        .find_one(doc! {"_id":previous.id.as_str()})
        .session(&mut *session)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let lease = next
        .work
        .lease
        .as_ref()
        .ok_or_else(|| mongodb::error::Error::custom("promoted mint claim is absent"))?;
    let intent = CredentialIssuance {
        operation_id: format!("mint_{}", uuid::Uuid::new_v4().simple()),
        sandbox_id: next.id.clone(),
        owner: next.owner.clone(),
        generation: next.credential.next_generation().ok_or_else(|| {
            mongodb::error::Error::custom("legacy credential generation overflow")
        })?,
        identity: IssuanceIdentity::LegacyUnobserved,
        owner_authority: None,
        original_holder: lease.holder.clone(),
        original_fence: lease.fence,
        created_at: next.updated_at.clone(),
        next_check_at: next.updated_at.clone(),
        state: IssuanceState::Unknown,
    };
    let marked = store
        .sandboxes
        .update_one(
            doc! {"_id":previous.id.as_str(),
            "credential_issuance_fence":{"$exists":false}},
            doc! {"$set":{"credential_issuance_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if marked.matched_count != 1 {
        return Err(mongodb::error::Error::custom(
            "legacy issuance marker is inconsistent",
        ));
    }
    rows.insert_one(document(&intent).map_err(mongodb::error::Error::custom)?)
        .session(session)
        .await?;
    Ok(())
}

fn may_have_minted(sandbox: &Sandbox) -> bool {
    sandbox.credential.source.is_managed()
        && match sandbox.work.step {
            SandboxWorkStep::MintInitialCredential | SandboxWorkStep::MintRotatedCredential => true,
            SandboxWorkStep::RunLifecycleOperation => {
                sandbox.needs_service_access_credential()
                    && matches!(
                        &sandbox.lifecycle.operation,
                        Some(
                            SandboxLifecycleOperation::Revive {
                                phase: SandboxRevivePhase::CredentialDelivery,
                                ..
                            } | SandboxLifecycleOperation::ImageRotation {
                                phase: SandboxImageRotationPhase::CredentialDelivery,
                                ..
                            }
                        )
                    )
            }
            _ => false,
        }
}
