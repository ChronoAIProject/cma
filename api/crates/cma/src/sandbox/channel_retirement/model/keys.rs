use super::*;
use crate::sandbox::{
    channel_lifecycle_credentials::model::CredentialPhase,
    channel_renewal::model::{credential_id, RenewalPhase},
};
use std::collections::BTreeMap;

impl ChannelRetirementSnapshot {
    pub(super) fn initial_keys(&self) -> Result<Vec<RetirementKey>, AllocationError> {
        let credential = &self.sandbox.credential;
        let mut known = BTreeMap::<String, SandboxCredentialAuthority>::new();
        let mut retain = |key: &SandboxCredentialAuthority| -> Result<(), AllocationError> {
            if known
                .get(&key.provider_key_id)
                .is_some_and(|old| old != key)
            {
                return Err(AllocationError::IdentityMismatch);
            }
            known.insert(key.provider_key_id.clone(), key.clone());
            Ok(())
        };
        if let Some(state) = &self.renewal {
            if state.schema_version != 1
                || state.revision == 0
                || state.allocation_id != self.allocation.id
                || state.generation < 2
                || state.previous.generation.checked_add(1) != Some(state.generation)
                || state.credential_id != self.name_for(state.generation)?
                || state.previous_credential_id != self.name_for(state.previous.generation)?
                || state.phase == RenewalPhase::MintStarted
                || state.claim.fence == 0
                || state.replacement.as_ref().is_none_or(|key| {
                    key.generation != state.generation
                        || key.provider_key_id == state.previous.provider_key_id
                })
            {
                return Err(AllocationError::IdentityMismatch);
            }
            if let Some(intent) = &state.mint_intent {
                intent
                    .require_material(state.replacement.as_ref().ok_or(AllocationError::Corrupt)?)
                    .map_err(|_| AllocationError::IdentityMismatch)?;
            }
            retain(&state.previous)?;
            retain(state.replacement.as_ref().ok_or(AllocationError::Corrupt)?)?;
        }
        if let Some(state) = &self.lifecycle {
            state.validate()?;
            if state.allocation_id != self.allocation.id
                || state.phase == CredentialPhase::MintStarted
                || state.credential_id != self.name_for(state.generation)?
                || state.previous_credential_id != self.name_for(state.previous.generation)?
            {
                return Err(AllocationError::IdentityMismatch);
            }
            retain(&state.previous)?;
            retain(state.replacement.as_ref().ok_or(AllocationError::Corrupt)?)?;
        }
        let mut keys = BTreeMap::new();
        for key in [credential.current.as_ref(), credential.pending.as_ref()]
            .into_iter()
            .flatten()
        {
            if (key.generation > 1 || known.contains_key(&key.provider_key_id))
                && known.get(&key.provider_key_id) != Some(key)
            {
                return Err(AllocationError::IdentityMismatch);
            }
            if keys
                .insert(key.provider_key_id.clone(), key.clone())
                .is_some()
            {
                return Err(AllocationError::IdentityMismatch);
            }
        }
        for debt in &credential.revoke_debt {
            let key = known
                .get(&debt.provider_key_id)
                .ok_or(AllocationError::IdentityMismatch)?;
            if keys
                .insert(key.provider_key_id.clone(), key.clone())
                .is_some()
            {
                return Err(AllocationError::IdentityMismatch);
            }
        }
        if keys.is_empty() || keys.len() > 3 {
            return Err(AllocationError::IdentityMismatch);
        }
        let mut result = Vec::new();
        let mut generations = std::collections::BTreeSet::new();
        for key in keys.into_values() {
            if key.generation == 0
                || key.generation > credential.key_generation
                || !generations.insert(key.generation)
                || key.provider_key_id == self.allocation.config.connector.key_id
            {
                return Err(AllocationError::IdentityMismatch);
            }
            result.push(RetirementKey {
                credential_id: self.name_for(key.generation)?,
                authority: key,
                revoked: false,
            });
        }
        result.sort_by_key(|key| key.authority.generation);
        Ok(result)
    }
    fn name_for(&self, generation: u64) -> Result<String, AllocationError> {
        if generation == 1 {
            self.provisioning
                .mint_credential_id
                .clone()
                .ok_or(AllocationError::Corrupt)
        } else {
            Ok(credential_id(&self.sandbox.id, generation))
        }
    }
}
