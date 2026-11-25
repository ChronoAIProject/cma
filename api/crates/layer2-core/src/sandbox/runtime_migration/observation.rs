//! Bounded server-owned forward verifier; public responses contain no source rows.
use crate::sandbox::store::{
    runtime_migration_now_ms, RuntimeActivityObservation, RuntimeActivityProof,
    RuntimeActivitySnapshot, RuntimeActivitySourceContext, RuntimeActivityUnknownReason,
    SandboxRuntimeActivityObservationStore, SandboxStoreError,
};
use crate::SandboxId;
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RuntimeActivityStatus {
    Unknown {
        reason: RuntimeActivityUnknownReason,
    },
    Active,
    Observing,
    Quiet {
        proof_reference: String,
    },
}

struct Entry {
    initial: RuntimeActivitySnapshot,
    last: RuntimeActivitySnapshot,
    proof: Option<(String, RuntimeActivityProof)>,
}

pub struct RuntimeActivityVerifier {
    store: Arc<dyn SandboxRuntimeActivityObservationStore>,
    context: Option<RuntimeActivitySourceContext>,
    scope: Vec<SandboxId>,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl RuntimeActivityVerifier {
    pub fn new(
        store: Arc<dyn SandboxRuntimeActivityObservationStore>,
        context: Option<RuntimeActivitySourceContext>,
        scope: Vec<SandboxId>,
    ) -> Self {
        Self {
            store,
            context,
            scope,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    pub async fn observe(
        &self,
        id: &SandboxId,
    ) -> Result<RuntimeActivityStatus, SandboxStoreError> {
        let Some(context) = self.context.as_ref().filter(|_| {
            self.scope.contains(id) && self.scope.len() <= super::MAX_RUNTIME_MIGRATION_ENTRIES
        }) else {
            return Ok(RuntimeActivityStatus::Unknown {
                reason: RuntimeActivityUnknownReason::UnqualifiedSource,
            });
        };
        // One bounded capture at a time also prevents a late earlier snapshot
        // from overwriting a newer baseline. This lock never mutates Mongo.
        let mut entries = self.entries.lock().await;
        let observation = match self.store.observe_runtime_activity(id, context).await {
            Ok(observation) => observation,
            Err(error) => {
                entries.remove(id.as_str());
                return Err(error);
            }
        };
        let snapshot = match observation {
            RuntimeActivityObservation::Unknown(reason) => {
                entries.remove(id.as_str());
                return Ok(RuntimeActivityStatus::Unknown { reason });
            }
            RuntimeActivityObservation::Snapshot(snapshot) => *snapshot,
        };
        if snapshot.validate().is_err() || &snapshot.source != context {
            entries.remove(id.as_str());
            return Ok(RuntimeActivityStatus::Unknown {
                reason: RuntimeActivityUnknownReason::UnsupportedContext,
            });
        }
        if snapshot.has_activity() {
            entries.remove(id.as_str());
            return Ok(RuntimeActivityStatus::Active);
        }
        let now = runtime_migration_now_ms()?;
        let Some(entry) = entries.get_mut(id.as_str()) else {
            if entries.len() >= super::MAX_RUNTIME_MIGRATION_ENTRIES {
                return Ok(RuntimeActivityStatus::Unknown {
                    reason: RuntimeActivityUnknownReason::CardinalityExceeded,
                });
            }
            entries.insert(
                id.to_string(),
                Entry {
                    initial: snapshot.clone(),
                    last: snapshot,
                    proof: None,
                },
            );
            return Ok(RuntimeActivityStatus::Observing);
        };
        if snapshot.vector != entry.last.vector
            || snapshot.desktop != entry.last.desktop
            || snapshot.source != entry.last.source
            || snapshot.storage_incarnation != entry.last.storage_incarnation
            || snapshot.watermark < entry.last.watermark
            || snapshot.capture_started_at_ms < entry.last.capture_completed_at_ms
        {
            *entry = Entry {
                initial: snapshot.clone(),
                last: snapshot,
                proof: None,
            };
            return Ok(RuntimeActivityStatus::Unknown {
                reason: RuntimeActivityUnknownReason::SourceChanged,
            });
        }
        // Reuse an original still-fresh proof only after this new coherent
        // observation confirms its complete vector. No timestamp is rebased.
        if let Some((reference, proof)) = &entry.proof {
            if proof.compare_current(&snapshot, now).is_ok() {
                entry.last = snapshot;
                return Ok(RuntimeActivityStatus::Quiet {
                    proof_reference: reference.clone(),
                });
            }
        }
        let proof = RuntimeActivityProof {
            schema_version: 1,
            initial: entry.initial.clone(),
            final_observation: snapshot.clone(),
        };
        entry.last = snapshot;
        let reference = uuid::Uuid::new_v4().to_string();
        let quiet = proof.validate_at(now).is_ok();
        entry.proof = Some((reference.clone(), proof));
        if quiet {
            Ok(RuntimeActivityStatus::Quiet {
                proof_reference: reference,
            })
        } else {
            Ok(RuntimeActivityStatus::Observing)
        }
    }

    /// A lookup handle is never bearer permission. The operator authority still
    /// binds this proof to exact ID/key/identity/target and the transaction rechecks.
    pub async fn proof_for(
        &self,
        id: &SandboxId,
        reference: &str,
    ) -> Result<RuntimeActivityProof, SandboxStoreError> {
        let entries = self.entries.lock().await;
        let (saved, proof) = entries
            .get(id.as_str())
            .and_then(|entry| entry.proof.as_ref())
            .ok_or(SandboxStoreError::Conflict)?;
        if saved != reference {
            return Err(SandboxStoreError::Conflict);
        }
        proof.validate_at(runtime_migration_now_ms()?)?;
        Ok(proof.clone())
    }
}
