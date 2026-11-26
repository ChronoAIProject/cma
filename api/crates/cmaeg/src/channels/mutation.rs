//! Exact channel intent and completed steps survive process loss; unknown sends never replay.
use super::management::{seal_definition, ManagementError};
use crate::records::{RecordClient, RecordError};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::{ChannelDefinition, ChannelMutation, ChannelMutationKind},
    seal::MasterKeyRing,
};

pub(crate) fn enabled_from_env() -> Result<bool, crate::ConfigError> {
    crate::config::channel_mutation_custody_enabled()
}
pub(super) fn available(
    definition: &ChannelDefinition,
    version: u64,
) -> Result<(), ManagementError> {
    if definition.mutation.is_some() {
        return Err(conflict(version));
    }
    Ok(())
}
fn conflict(version: u64) -> ManagementError {
    ManagementError::Conflict {
        expected: version,
        actual: Some(version),
    }
}
pub(super) fn fingerprint(definition: &ChannelDefinition) -> String {
    let mut definition = definition.clone();
    definition.mutation = None;
    super::definition::hash_secret(&cmaeg_core::profile::canonical_json(
        definition.to_blob_json(),
    ))
}
pub(super) fn original_version(definition: &ChannelDefinition, version: u64) -> u64 {
    definition
        .mutation
        .as_ref()
        .map_or(version, |held| held.original_version)
}
pub(super) struct Intent<'a> {
    pub operation: ChannelMutationKind,
    pub expected_version: u64,
    pub fingerprint: String,
    pub desired: &'a ChannelDefinition,
}
pub(super) struct Reservation {
    original: ChannelDefinition,
    retained: ChannelDefinition,
    held_blob: Vec<u8>,
    pub(super) desired: ChannelDefinition,
    pub(super) version: u64,
    held: bool,
    completed: Vec<String>,
    active: Option<String>,
}
impl Reservation {
    pub(super) async fn acquire(
        records: &RecordClient,
        keys: &MasterKeyRing,
        current: &ChannelDefinition,
        version: u64,
        intent: Intent<'_>,
    ) -> Result<Self, ManagementError> {
        let mut original = current.clone();
        original.mutation = None;
        let mut desired = intent.desired.clone();
        desired.mutation = None;
        let mut completed = Vec::new();
        let held = records.channel_mutation_custody() || current.mutation.is_some();
        if let Some(previous) = &current.mutation {
            if previous.operation != intent.operation
                || previous.original_version != intent.expected_version
                || previous.fingerprint != intent.fingerprint
                || (previous.effect.is_some()
                    && intent.operation != ChannelMutationKind::ProfileRevision)
            {
                return Err(conflict(version));
            }
            let bytes = STANDARD
                .decode(&previous.intended)
                .map_err(|_| conflict(version))?;
            let opened =
                cmaeg_core::seal::open_blob(keys, &bytes).map_err(|_| conflict(version))?;
            desired = serde_json::from_slice(&opened.payload).map_err(|_| conflict(version))?;
            if desired.mutation.is_some()
                || desired.record_id != current.record_id
                || desired.owner != current.owner
            {
                return Err(conflict(version));
            }
            desired.validate().map_err(ManagementError::Invalid)?;
            if let Some(effect) = &previous.effect {
                // Profile preparation uses a deterministic new binding identity
                // and the exact retained profile and credential. The provider PUT
                // is idempotent under that identity, so an uncertain reply can
                // be reconciled without sending a different effect.
                if effect
                    != &format!(
                        "bind:{}:{}",
                        desired.provider.slug, desired.provider.binding_id
                    )
                {
                    return Err(conflict(version));
                }
            }
            completed = previous.completed.clone();
        } else if version != intent.expected_version {
            return Err(conflict(version));
        }
        let mut reservation = Self {
            original,
            retained: current.clone(),
            held_blob: Vec::new(),
            desired,
            version,
            held,
            completed,
            active: None,
        };
        if held {
            // A fresh nonce makes a paused previous safe-phase claimant lose its
            // next admission CAS. It cannot send after another exact retry wins.
            reservation.retained.mutation = Some(ChannelMutation {
                nonce: uuid::Uuid::new_v4().to_string(),
                operation_id: current
                    .mutation
                    .as_ref()
                    .map(|value| value.operation_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                operation: intent.operation,
                original_version: intent.expected_version,
                fingerprint: intent.fingerprint,
                intended: STANDARD.encode(seal_definition(keys, &reservation.desired)?),
                completed: reservation.completed.clone(),
                effect: None,
            });
            reservation
                .retained
                .validate()
                .map_err(ManagementError::Invalid)?;
            let blob = seal_definition(keys, &reservation.retained)?;
            match records
                .put(
                    &current.record_id,
                    &current.owner.subject,
                    &blob,
                    Some(version),
                )
                .await
            {
                Ok(next) => reservation.version = next,
                Err(error) => {
                    let actual = records
                        .get(records.record_url(current.record_id.as_str())?.as_str())
                        .await?;
                    if Some(actual.version) != version.checked_add(1) || actual.blob != blob {
                        return Err(error.into());
                    }
                    reservation.version = actual.version;
                }
            }
            reservation.held_blob = blob;
        }
        Ok(reservation)
    }
    pub(super) async fn prepare(
        &mut self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        desired: ChannelDefinition,
    ) -> Result<(), ManagementError> {
        if self.started() && fingerprint(&self.desired) != fingerprint(&desired) {
            return Err(conflict(self.version));
        }
        if self.held {
            let mut retained = self.retained.clone();
            retained.mutation.as_mut().unwrap().intended =
                STANDARD.encode(seal_definition(keys, &desired)?);
            self.persist(records, keys, retained).await?;
        }
        self.desired = desired;
        Ok(())
    }
    pub(super) fn done(&self, identity: &str) -> bool {
        self.completed.iter().any(|step| step == identity)
    }
    pub(super) fn started(&self) -> bool {
        self.active.is_some() || !self.completed.is_empty()
    }
    pub(super) async fn effect(
        &mut self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        identity: String,
    ) -> Result<(), ManagementError> {
        if self.active.is_some() || self.done(&identity) {
            return Err(conflict(self.version));
        }
        self.active = Some(identity.clone());
        if self.held {
            let mut retained = self.retained.clone();
            retained.mutation.as_mut().unwrap().effect = Some(identity);
            self.persist(records, keys, retained).await?;
        }
        Ok(())
    }
    /// Called only with a confirmed result; target includes the negotiated result.
    pub(super) async fn confirmed(
        &mut self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        desired: ChannelDefinition,
    ) -> Result<(), ManagementError> {
        let identity = self.active.clone().ok_or_else(|| conflict(self.version))?;
        let mut completed = self.completed.clone();
        completed.push(identity);
        if self.held {
            let mut retained = self.retained.clone();
            let mutation = retained.mutation.as_mut().unwrap();
            mutation.effect = None;
            mutation.completed = completed.clone();
            mutation.intended = STANDARD.encode(seal_definition(keys, &desired)?);
            self.persist(records, keys, retained).await?;
        }
        self.desired = desired;
        self.completed = completed;
        self.active = None;
        Ok(())
    }
    /// Terminal refusal/no-send permits the exact request to resume this step.
    pub(super) async fn retryable(
        &mut self,
        records: &RecordClient,
        keys: &MasterKeyRing,
    ) -> Result<(), ManagementError> {
        if self.held {
            let mut retained = self.retained.clone();
            retained.mutation.as_mut().unwrap().effect = None;
            self.persist(records, keys, retained).await?;
        }
        self.active = None;
        Ok(())
    }
    async fn persist(
        &mut self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        retained: ChannelDefinition,
    ) -> Result<(), ManagementError> {
        retained.validate().map_err(ManagementError::Invalid)?;
        let blob = seal_definition(keys, &retained)?;
        let version = self.write_exact(records, &blob).await?;
        self.retained = retained;
        self.held_blob = blob;
        self.version = version;
        Ok(())
    }
    async fn write_exact(
        &self,
        records: &RecordClient,
        blob: &[u8],
    ) -> Result<u64, ManagementError> {
        for _ in 0..3 {
            match records
                .put(
                    &self.original.record_id,
                    &self.original.owner.subject,
                    blob,
                    Some(self.version),
                )
                .await
            {
                Ok(version) => return Ok(version),
                Err(error) => {
                    let current = records
                        .get(
                            records
                                .record_url(self.original.record_id.as_str())?
                                .as_str(),
                        )
                        .await?;
                    if Some(current.version) == self.version.checked_add(1) && current.blob == blob
                    {
                        return Ok(current.version);
                    }
                    if !self.held
                        || current.version != self.version
                        || current.blob != self.held_blob
                    {
                        return Err(error.into());
                    }
                }
            }
        }
        Err(RecordError::Unavailable("channel custody settlement remains pending".into()).into())
    }
    pub(super) async fn publish(
        &self,
        records: &RecordClient,
        keys: &MasterKeyRing,
        next: &ChannelDefinition,
    ) -> Result<(Vec<u8>, u64), ManagementError> {
        if self.active.is_some() {
            return Err(conflict(self.version));
        }
        let mut next = next.clone();
        next.mutation = None;
        next.version = self.version;
        let blob = seal_definition(keys, &next)?;
        let version = self.write_exact(records, &blob).await?;
        Ok((blob, version))
    }
    pub(super) async fn release_unsent(
        &self,
        records: &RecordClient,
        keys: &MasterKeyRing,
    ) -> Result<(), ManagementError> {
        if self.started() {
            return Err(conflict(self.version));
        }
        if self.held {
            self.publish(records, keys, &self.original).await?;
        }
        Ok(())
    }
    pub(super) async fn delete(&self, records: &RecordClient) -> Result<(), ManagementError> {
        if self.active.is_some() {
            return Err(conflict(self.version));
        }
        for _ in 0..3 {
            match records.delete(&self.original.record_id, self.version).await {
                Ok(_) => return Ok(()),
                Err(error) => match records
                    .get(
                        records
                            .record_url(self.original.record_id.as_str())?
                            .as_str(),
                    )
                    .await
                {
                    Err(RecordError::NotFound) => return Ok(()),
                    Ok(current)
                        if self.held
                            && current.version == self.version
                            && current.blob == self.held_blob => {}
                    _ => return Err(error.into()),
                },
            }
        }
        Err(RecordError::Unavailable("channel deletion remains pending".into()).into())
    }
}
impl From<RecordError> for ManagementError {
    fn from(error: RecordError) -> Self {
        Self::Record(error)
    }
}
