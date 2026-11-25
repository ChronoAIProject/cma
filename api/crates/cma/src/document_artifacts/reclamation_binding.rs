//! Bind physical source custody to the exact installed quota inventory.
use super::quota;
use layer2_core::document_artifacts::{valid_digest, DocumentError};
use mongodb::bson::{doc, DateTime, Document};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Binding {
    pub(super) activation: String,
    pub(super) inventory: String,
    pub(super) namespace: String,
    pub(super) bound_at: DateTime,
}
impl Binding {
    pub(super) fn read(state: &Document, namespace: &str) -> Result<Self, DocumentError> {
        if !quota::ready(state)
            || !valid_digest(namespace)
            || state.get_str("storage_namespace_sha256").ok() != Some(namespace)
        {
            return Err(DocumentError::Unavailable);
        }
        Ok(Self {
            activation: state
                .get_str("activation_id")
                .map_err(|_| DocumentError::Corrupt)?
                .into(),
            inventory: state
                .get_str("inventory_sha256")
                .map_err(|_| DocumentError::Corrupt)?
                .into(),
            namespace: namespace.into(),
            bound_at: *state
                .get_datetime("storage_namespace_bound_at")
                .map_err(|_| DocumentError::Corrupt)?,
        })
    }
}

impl quota::MongoDocumentQuotaStore {
    /// Operator attestation is explicit and immutable for an installed baseline.
    pub(super) async fn bind_storage(
        &self,
        activation: &str,
        inventory: &str,
        namespace: &str,
    ) -> Result<(), DocumentError> {
        if !super::quota_baseline::valid_activation(activation)
            || !valid_digest(inventory)
            || !valid_digest(namespace)
        {
            return Err(DocumentError::Invalid);
        }
        let state = self
            .rows
            .find_one(doc! {"_id":quota::STATE})
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?
            .ok_or(DocumentError::Unavailable)?;
        if !quota::ready(&state)
            || state.get_str("activation_id").ok() != Some(activation)
            || state.get_str("inventory_sha256").ok() != Some(inventory)
        {
            return Err(DocumentError::Conflict);
        }
        let result = self.rows.update_one(doc! {"_id":quota::STATE, "phase":"ready", "activation_id":activation, "inventory_sha256":inventory,
            "$or":[{"storage_namespace_sha256":{"$exists":false}},{"storage_namespace_sha256":namespace}]}, vec![doc! {"$set":{"storage_namespace_sha256":namespace, "source_generations":true, "storage_namespace_bound_at":{"$ifNull":["$storage_namespace_bound_at","$$NOW"]}}}]).await.map_err(quota::unavailable)?;
        if result.matched_count != 1 {
            return Err(DocumentError::Conflict);
        }
        let bound = self
            .rows
            .find_one(doc! {"_id":quota::STATE})
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?
            .ok_or(DocumentError::Unavailable)?;
        let verified = Binding::read(&bound, namespace)?;
        if verified.activation != activation
            || verified.inventory != inventory
            || !quota::generation_enabled(&bound)?
        {
            return Err(DocumentError::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn verify_storage(
        &self,
        namespace: Option<&str>,
        required: bool,
    ) -> Result<(), DocumentError> {
        let state = self
            .rows
            .find_one(doc! {"_id":quota::STATE})
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?;
        if let Some(state) = &state {
            if required || state.contains_key("storage_namespace_sha256") {
                Binding::read(state, namespace.ok_or(DocumentError::Unavailable)?)?;
            }
        } else if required {
            return Err(DocumentError::Unavailable);
        }
        Ok(())
    }
}
