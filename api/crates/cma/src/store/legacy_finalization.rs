//! Read-only validation of final retirement receipts written by the retired bridge.
use layer2_core::profiles::content_digest;
use layer2_core::sandbox::{Sandbox, SandboxStatus, SandboxWorkStep};
use mongodb::bson::Document;
use serde::{Deserialize, Serialize};
mod model;
use model::HistorySeal;

#[cfg(test)]
mod tests;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u16,
    allocation_id: String,
    allocation_operation_id: String,
    sandbox_sha256: String,
    provider_finality_sha256: String,
    retirement_state_sha256: String,
    storage_audit_id: String,
    storage_audit_sha256: String,
    history: HistorySeal,
    retired_at: String,
}

pub(super) fn validate_document(document: &Document, sandbox: &Sandbox) -> Result<(), ()> {
    let Some(value) = document.get("channel_retirement_finalization") else {
        return if document.contains_key("channel_history_seal") {
            Err(())
        } else {
            Ok(())
        };
    };
    let receipt: Receipt = mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| ())?;
    if receipt.schema_version != 1
        || receipt.history.policy.allocation_id != receipt.allocation_id
        || receipt.history.sandbox_id != sandbox.id.as_str()
        || receipt.history.sandbox_created_at != sandbox.created_at
        || receipt.sandbox_sha256
            != content_digest(&("channel-retirement-finalization-v1", sandbox)).map_err(|_| ())?
        || sandbox.status != SandboxStatus::Retired
        || sandbox.retired_at.as_ref() != Some(&receipt.retired_at)
        || receipt.history.sealed_at != receipt.retired_at
        || sandbox.purge_after.is_some()
        || sandbox.runtime.is_some()
        || sandbox.lifecycle.home.is_some()
        || sandbox.work.step != SandboxWorkStep::None
        || sandbox.work.lease.is_some()
        || sandbox.credential.current.is_some()
        || sandbox.credential.pending.is_some()
        || !sandbox.credential.revoke_debt.is_empty()
        || [
            &receipt.provider_finality_sha256,
            &receipt.retirement_state_sha256,
            &receipt.storage_audit_sha256,
        ]
        .iter()
        .any(|v| {
            v.len() != 64
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        || document
            .get_str("channel_retirement_finalization_sha256")
            .ok()
            != Some(
                content_digest(&("channel-retirement-finalization-v1", &receipt))
                    .map_err(|_| ())?
                    .as_str(),
            )
        || document.get_str("channel_history_seal").ok()
            != Some(
                content_digest(&("channel-retained-history-seal-v1", &receipt.history))
                    .map_err(|_| ())?
                    .as_str(),
            )
    {
        return Err(());
    }
    Ok(())
}
