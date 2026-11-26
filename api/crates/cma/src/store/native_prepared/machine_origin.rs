use layer2_core::sandbox::{Sandbox, SandboxStoreError, WarmSandboxState};
use mongodb::{bson::doc, ClientSession};

use super::{read, MongoSandboxStore};
use crate::store::sandbox_events::TransactionEventError::{self, Domain, Driver};

pub(in crate::store) async fn verify_machine_origin(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    operation_id: &str,
    session: &mut ClientSession,
) -> Result<(), TransactionEventError> {
    let Some(document) = store
        .native_prepared
        .find_one(doc! {"_id": sandbox.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(Driver)?
    else {
        return Ok(());
    };
    let record = read(&document).map_err(Domain)?;
    let proved = record.payload.id == sandbox.id
        && record.payload.state == WarmSandboxState::Claimed
        && record.plan.machine()
        && record.ownership.is_some_and(|owned| {
            owned.owner == sandbox.owner
                && owned.operation_id == operation_id
                && owned.sandbox == sandbox.profile
        })
        && record
            .payload
            .home
            .as_deref()
            .zip(sandbox.lifecycle.home.as_deref())
            .is_some_and(|(original, retained)| {
                original.request.volume_id == retained.request.volume_id
                    && original
                        .receipt
                        .as_ref()
                        .zip(retained.receipt.as_ref())
                        .is_some_and(|(old, current)| {
                            old.storage_reference == current.storage_reference
                                && old.storage_generation == current.storage_generation
                        })
                    && original
                        .native_pool
                        .as_ref()
                        .and_then(|pool| pool.claim.as_ref())
                        .zip(
                            retained
                                .native_pool
                                .as_ref()
                                .and_then(|pool| pool.claim.as_ref()),
                        )
                        .is_some_and(|(old, current)| {
                            old.uid == current.uid && old.request == current.request
                        })
            });
    if proved {
        Ok(())
    } else {
        Err(Domain(SandboxStoreError::InvalidTransaction(
            "prepared Machine ownership is unproved",
        )))
    }
}
