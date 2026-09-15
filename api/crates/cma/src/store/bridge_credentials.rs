//! Local encrypted custody for exactly selected bridge credentials. Both import
//! and revocation fence the current Bridge definition in the same transaction.

pub(crate) mod control_plane;

use layer2_core::bridge::{
    BridgeCustodyPin, BridgeError, BridgeManagementLease, BridgeRecord, BridgeSecretRef,
    BridgeSetupBlock, BridgeState,
};
use mongodb::bson::{doc, Document};
use mongodb::{Client, ClientSession, Collection};

use super::durability::majority_collection;
use super::encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, record_bson, require_version,
    start_transaction, CommitOutcome, DOCUMENT_SCHEMA_VERSION,
};
use crate::auth::persistence::SecretValue;
use crate::bridge::custody::{
    BridgeCredentialStore, BridgeCustodyMetadata, StoredBridgeCredential,
};

pub(crate) struct MongoBridgeCredentialStore {
    client: Client,
    definitions: Collection<Document>,
    credentials: Collection<Document>,
    cipher: AuthCipher,
}

impl MongoBridgeCredentialStore {
    /// Read the exact encrypted custody in the caller's transaction. The caller
    /// must also write the Bridge definition so concurrent revocation conflicts;
    /// retries then observe the revoked revision instead of reusing stale proof.
    pub(crate) async fn require_binding(
        &self,
        bridge: &BridgeRecord,
        pin: &BridgeCustodyPin,
        session: &mut ClientSession,
    ) -> Result<(), BridgeError> {
        self.require_retained_binding(bridge, &bridge.current()?.config, pin, session)
            .await
    }

    /// Accepted work may retain an older exact generation under an explicit
    /// drain policy. This still resolves only that generation's selected key.
    pub(crate) async fn require_retained_binding(
        &self,
        bridge: &BridgeRecord,
        config: &layer2_core::bridge::BridgeConfig,
        pin: &BridgeCustodyPin,
        session: &mut ClientSession,
    ) -> Result<(), BridgeError> {
        let unavailable = || BridgeError::ConnectionRequired(BridgeSetupBlock::CustodyUnavailable);
        if pin.reference() != &config.connector.custody
            || !bridge
                .generations
                .iter()
                .any(|generation| generation.config == *config)
        {
            return Err(unavailable());
        }
        let document = self
            .credentials
            .find_one(doc! { "_id": pin.reference().as_str() })
            .session(session)
            .await
            .map_err(|_| BridgeError::Unavailable)?
            .ok_or_else(unavailable)?;
        let material = self.decode(document)?;
        let metadata = material.metadata;
        if metadata.bridge_id != bridge.id
            || metadata.owner != bridge.owner
            || metadata.connector != config.connector
            || metadata.revision != pin.revision()
            || metadata.key_state_version != pin.key_state_version()
            || metadata.revoked_at.is_some()
        {
            return Err(unavailable());
        }
        Ok(())
    }

    pub(super) async fn open(
        client: &Client,
        name: &str,
        deployment: &str,
        key: &AuthEncryptionKey,
    ) -> Result<Self, layer2_core::StoreError> {
        let database = client.database(name);
        super::schema::initialize_collections(&database, &["bridge_credentials"]).await?;
        Ok(Self {
            client: client.clone(),
            definitions: majority_collection(&database, "bridge_definitions"),
            credentials: majority_collection(&database, "bridge_credentials"),
            cipher: AuthCipher::new(deployment, key)?,
        })
    }

    fn encode(&self, material: &StoredBridgeCredential) -> Result<Document, BridgeError> {
        material.metadata.validate()?;
        let aad = serde_json::to_vec(&material.metadata).map_err(|_| BridgeError::Corrupt)?;
        let encrypted = self
            .cipher
            .seal(
                "bridge-credential",
                &aad,
                "selected-key",
                material.value.expose(),
            )
            .map_err(|_| BridgeError::Unavailable)?;
        let mut document = doc! { "_id": material.metadata.connector.custody.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "metadata": record_bson(&material.metadata).map_err(|_| BridgeError::Corrupt)?,
        "secret": record_bson(&encrypted).map_err(|_| BridgeError::Corrupt)? };
        if let Some(grant) = &material.control_plane {
            validate_grant(grant, &material.metadata)?;
            let clear = zeroize::Zeroizing::new(
                serde_json::to_string(grant).map_err(|_| BridgeError::Corrupt)?,
            );
            let encrypted = self
                .cipher
                .seal("bridge-control-plane", &aad, "grant", &clear)
                .map_err(|_| BridgeError::Unavailable)?;
            document.insert(
                "control_plane",
                record_bson(&encrypted).map_err(|_| BridgeError::Corrupt)?,
            );
        }
        Ok(document)
    }

    fn decode(&self, document: Document) -> Result<StoredBridgeCredential, BridgeError> {
        require_version(&document).map_err(|_| BridgeError::Corrupt)?;
        let metadata: BridgeCustodyMetadata = mongodb::bson::deserialize_from_bson(
            document
                .get("metadata")
                .cloned()
                .ok_or(BridgeError::Corrupt)?,
        )
        .map_err(|_| BridgeError::Corrupt)?;
        metadata.validate()?;
        if document.get_str("_id").ok() != Some(metadata.connector.custody.as_str()) {
            return Err(BridgeError::Corrupt);
        }
        let encrypted: CipherEnvelope = mongodb::bson::deserialize_from_bson(
            document
                .get("secret")
                .cloned()
                .ok_or(BridgeError::Corrupt)?,
        )
        .map_err(|_| BridgeError::Corrupt)?;
        let aad = serde_json::to_vec(&metadata).map_err(|_| BridgeError::Corrupt)?;
        let value = SecretValue::new(
            self.cipher
                .open("bridge-credential", &aad, "selected-key", encrypted)
                .map_err(|_| BridgeError::Corrupt)?,
        );
        let control_plane = document
            .get("control_plane")
            .map(|encrypted| {
                let encrypted: CipherEnvelope =
                    mongodb::bson::deserialize_from_bson(encrypted.clone())
                        .map_err(|_| BridgeError::Corrupt)?;
                let clear = zeroize::Zeroizing::new(
                    self.cipher
                        .open("bridge-control-plane", &aad, "grant", encrypted)
                        .map_err(|_| BridgeError::Corrupt)?,
                );
                let grant = serde_json::from_str(&clear).map_err(|_| BridgeError::Corrupt)?;
                validate_grant(&grant, &metadata)?;
                Ok::<_, BridgeError>(grant)
            })
            .transpose()?;
        Ok(StoredBridgeCredential {
            metadata,
            value,
            control_plane,
        })
    }

    async fn apply(
        &self,
        management: &BridgeManagementLease,
        change: Change,
    ) -> Result<BridgeCustodyMetadata, BridgeError> {
        for _ in 0..8 {
            management.require_current()?;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| BridgeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeError::Unavailable)?;
            let result = match self.transaction(management, &change, &mut session).await {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        MutationError::Mongo(_) => return Err(BridgeError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    // Unknown commit remains a retryable availability failure;
                    // import identity and revocation key replay after restart.
                    return Err(BridgeError::Unavailable);
                }
            }
        }
        Err(BridgeError::Unavailable)
    }

    async fn transaction(
        &self,
        management: &BridgeManagementLease,
        change: &Change,
        session: &mut ClientSession,
    ) -> Result<BridgeCustodyMetadata, MutationError> {
        management.require_current()?;
        let definition = self
            .definitions
            .find_one(doc! { "_id": management.record().id.as_str() })
            .session(&mut *session)
            .await?
            .ok_or(BridgeError::NotFound)?;
        if super::bridge_definitions::decode_bridge(&definition)? != *management.record() {
            return Err(BridgeError::Conflict.into());
        }
        // Writing the same definition document makes concurrent configuration
        // changes conflict with this transaction; a snapshot read alone would
        // permit a credential/configuration write skew across separate rows.
        let fenced = self.definitions.update_one(
            doc! { "_id": management.record().id.as_str(), "revision": management.record().revision as i64 },
            doc! { "$set": { "credential_fence": uuid::Uuid::new_v4().to_string() } },
        ).session(&mut *session).await?;
        if fenced.matched_count != 1 {
            return Err(BridgeError::Conflict.into());
        }
        let reference = match change {
            Change::Admit(material) => &material.metadata.connector.custody,
            Change::Revoke { reference, .. } => reference,
        };
        let current = self
            .credentials
            .find_one(doc! { "_id": reference.as_str() })
            .session(&mut *session)
            .await?
            .map(|document| self.decode(document))
            .transpose()?;
        let existed = current.is_some();
        let material = match change {
            Change::Admit(material) => {
                let record = management.record();
                if !matches!(record.state, BridgeState::Disabled { .. })
                    || material.metadata.bridge_id != record.id
                    || material.metadata.owner != record.owner
                    || material.metadata.connector != record.current()?.config.connector
                    || material.metadata.created_by != *management.actor()
                    || material.metadata.revision != 1
                {
                    return Err(BridgeError::Conflict.into());
                }
                if material.control_plane.as_ref().is_some_and(|grant| {
                    !matches!(management.actor(),
                        layer2_core::resource_access::ResourceActor::Human { subject }
                        | layer2_core::resource_access::ResourceActor::Api { subject, delegation_id: None }
                        if subject == &grant.subject)
                }) {
                    return Err(BridgeError::Corrupt.into());
                }
                if let Some(current) = current {
                    let mut comparable = material.metadata.clone();
                    comparable.created_at = current.metadata.created_at.clone();
                    // The original selected-key importer remains immutable.
                    // Explicit new consent may come from another current org
                    // admin; its subject is recorded in the separate grant.
                    comparable.created_by = current.metadata.created_by.clone();
                    if current.metadata != comparable || current.value != material.value {
                        return Err(BridgeError::IdempotencyConflict.into());
                    }
                    management.require_current()?;
                    // An unchanged explicit request replays the original expiry.
                    // Deliberate reconnect may attach or renew an expired grant
                    // while the Bridge is disabled, under the same definition fence.
                    let renew = material.control_plane.as_ref().is_some_and(|next| {
                        current.control_plane.as_ref().is_none_or(|old| {
                                old.subject != next.subject
                                    || old.incarnation != next.incarnation
                                    || layer2_core::seconds_from_rfc3339(&old.expires_at)
                                        <= layer2_core::seconds_from_rfc3339(
                                            &layer2_core::now_rfc3339(),
                                        )
                            })
                    });
                    if !renew {
                        return Ok(current.metadata);
                    }
                    StoredBridgeCredential {
                        metadata: current.metadata,
                        value: current.value,
                        control_plane: material.control_plane.clone(),
                    }
                } else {
                    StoredBridgeCredential {
                        metadata: material.metadata.clone(),
                        value: material.value.clone(),
                        control_plane: material.control_plane.clone(),
                    }
                }
            }
            Change::Revoke {
                expected_revision,
                key_digest,
                ..
            } => {
                let mut current = current.ok_or(BridgeError::NotFound)?;
                if current.metadata.bridge_id != management.record().id
                    || current.metadata.owner != management.record().owner
                {
                    return Err(BridgeError::NotFound.into());
                }
                if current.metadata.revocation_key_digest.as_deref() == Some(key_digest.as_str()) {
                    management.require_current()?;
                    return Ok(current.metadata);
                }
                if current.metadata.revision != *expected_revision
                    || current.metadata.revoked_at.is_some()
                {
                    return Err(BridgeError::Conflict.into());
                }
                current.metadata.revision = 2;
                current.metadata.revoked_at = Some(layer2_core::now_rfc3339());
                current.metadata.revoked_by = Some(management.actor().clone());
                current.metadata.revocation_key_digest = Some(key_digest.clone());
                current
            }
        };
        material.metadata.validate()?;
        match change {
            Change::Admit(_) => {
                if existed {
                    self.credentials
                        .replace_one(
                            doc! { "_id": reference.as_str(), "metadata.revision": 1_i64 },
                            self.encode(&material)?,
                        )
                        .session(&mut *session)
                        .await?;
                } else {
                    self.credentials
                        .insert_one(self.encode(&material)?)
                        .session(&mut *session)
                        .await?;
                }
            }
            Change::Revoke {
                expected_revision, ..
            } => {
                let replaced = self.credentials.replace_one(doc! { "_id": reference.as_str(), "metadata.revision": *expected_revision as i64 }, self.encode(&material)?)
                    .session(&mut *session).await?;
                if replaced.matched_count != 1 {
                    return Err(BridgeError::Conflict.into());
                }
            }
        }
        management.require_current()?;
        Ok(material.metadata)
    }
}

#[async_trait::async_trait]
impl BridgeCredentialStore for MongoBridgeCredentialStore {
    async fn get(
        &self,
        reference: &BridgeSecretRef,
    ) -> Result<Option<StoredBridgeCredential>, BridgeError> {
        self.credentials
            .find_one(doc! { "_id": reference.as_str() })
            .await
            .map_err(|_| BridgeError::Unavailable)?
            .map(|document| self.decode(document))
            .transpose()
    }
    async fn admit(
        &self,
        management: &BridgeManagementLease,
        material: StoredBridgeCredential,
    ) -> Result<BridgeCustodyMetadata, BridgeError> {
        self.apply(management, Change::Admit(Box::new(material)))
            .await
    }
    async fn revoke(
        &self,
        management: &BridgeManagementLease,
        reference: &BridgeSecretRef,
        expected_revision: u64,
        key_digest: &str,
    ) -> Result<BridgeCustodyMetadata, BridgeError> {
        self.apply(
            management,
            Change::Revoke {
                reference: reference.clone(),
                expected_revision,
                key_digest: key_digest.into(),
            },
        )
        .await
    }
}

fn validate_grant(
    grant: &crate::bridge::control_plane::BridgeControlPlaneGrant,
    metadata: &BridgeCustodyMetadata,
) -> Result<(), BridgeError> {
    grant.validate()?;
    if grant.bridge_id != metadata.bridge_id
        || grant.owner != metadata.owner
        || grant.connector != metadata.connector
    {
        return Err(BridgeError::Corrupt);
    }
    Ok(())
}

enum Change {
    Admit(Box<StoredBridgeCredential>),
    Revoke {
        reference: BridgeSecretRef,
        expected_revision: u64,
        key_digest: String,
    },
}
#[derive(Debug, thiserror::Error)]
enum MutationError {
    #[error(transparent)]
    Core(#[from] BridgeError),
    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),
}
