//! A terminal purge may erase custody only while permanent creation identity remains.
use super::super::agent_log_integrity::TransactionLogError;
use super::*;
use layer2_core::sandbox::{store::SandboxStoreError, Sandbox};
use sha2::{Digest, Sha256};

pub(in crate::store) async fn require_finality(
    store: &MongoSandboxStore,
    rows: &[Document],
    sandbox: Option<&Sandbox>,
    session: &mut mongodb::ClientSession,
) -> Result<(), TransactionLogError> {
    let mut server_now = None;
    for row in rows {
        let effect = read(row).map_err(|_| SandboxStoreError::Conflict)?;
        let unresolved = effect.unknown();
        if unresolved || effect.accepted().is_some() {
            let id = row
                .get_str("_id")
                .map_err(|_| SandboxStoreError::Conflict)?;
            if unresolved && !capability_expired(store, &effect, &mut server_now).await? {
                let sandbox_sha256 = sandbox
                    .map(|sandbox| format!("{:x}", Sha256::digest(sandbox.id.as_str().as_bytes())));
                let lease_sha256 =
                    sandbox
                        .and_then(|sandbox| sandbox.work.lease.as_ref())
                        .map(|lease| {
                            let identity = format!("{}:{}", lease.fence, lease.holder);
                            format!("{:x}", Sha256::digest(identity.as_bytes()))
                        });
                tracing::warn!(
                    agent_sha256 = %format_args!("{:x}", Sha256::digest(id.as_bytes())),
                    sandbox_sha256 = sandbox_sha256.as_deref().unwrap_or_default(),
                    lease_sha256 = lease_sha256.as_deref().unwrap_or_default(),
                    "sandbox purge deferred: automatic title provider effect is within its claim deadline"
                );
                return Err(SandboxStoreError::Conflict.into());
            }
            let provenance = store
                .agent_creation_provenance
                .find_one(doc! {"_id": id})
                .session(&mut *session)
                .await
                .map_err(TransactionLogError::Driver)?
                .ok_or(SandboxStoreError::Conflict)?;
            super::super::agent_provenance::read(&provenance)?;
            // Production admission inserts this same permanent unique key; the
            // low-level creation port also refuses its reuse. Neither purge nor
            // any title/fallback/rollback writer deletes creation provenance.
        }
    }
    Ok(())
}

/// An unresolved effect is final once nothing can still accept its result
/// (#3275). Acceptance is fenced on server time before the claim deadline
/// that the retained intent records; a legacy attempt retains no capability.
/// Nothing resolves an unresolved effect, so without this a Sandbox whose
/// title request failed could never be purged. No redispatch is possible:
/// permanent creation provenance still prevents the identity's reuse.
async fn capability_expired(
    store: &MongoSandboxStore,
    effect: &AgentTitleEffectState,
    server_now: &mut Option<i64>,
) -> Result<bool, TransactionLogError> {
    let deadline = match effect {
        AgentTitleEffectState::LegacyUnknown => return Ok(true),
        AgentTitleEffectState::Retained(effect) if effect.result.is_none() => {
            i64::try_from(effect.intent.expires_ms).map_err(|_| SandboxStoreError::Conflict)?
        }
        _ => return Ok(false),
    };
    let now = match *server_now {
        Some(now) => now,
        None => {
            let reply = store
                .client
                .database(&store.agents.namespace().db)
                .run_command(doc! {"hello": 1})
                .await
                .map_err(TransactionLogError::Driver)?;
            let now = reply
                .get_datetime("localTime")
                .map_err(|_| SandboxStoreError::Conflict)?
                .timestamp_millis();
            *server_now = Some(now);
            now
        }
    };
    Ok(now >= deadline)
}
