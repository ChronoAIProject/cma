//! Retained provider revisions for a stable channel identity.
use super::{validation::validate_hash, ChannelDefinition, ProviderBinding, Secret};
use crate::{profile::EventWorkerProfile, ValidationError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// The sealed channel record is the publication authority. Assignment starts
/// while the original binding remains active, before any profile cutover.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRevisions {
    pub current_revision: u64,
    pub baseline_complete: bool,
    #[serde(default)]
    pub last_publication: Option<ProfilePublication>,
    pub retained: Vec<RetainedProfileRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePublication {
    pub expected_version: u64,
    pub request_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedProfileRevision {
    pub revision: u64,
    pub profile_digest: String,
    pub profile: Value,
    pub provider: ProviderBinding,
    pub credential: Secret,
}

impl ProfileRevisions {
    pub(super) fn private_json(&self) -> Value {
        json!({
            "current_revision": self.current_revision,
            "baseline_complete": self.baseline_complete,
            "last_publication": self.last_publication,
            "retained": self.retained.iter().map(|revision| json!({
                "revision": revision.revision,
                "profile_digest": revision.profile_digest,
                "profile": revision.profile,
                "provider": revision.provider,
                "credential": revision.credential.as_str(),
            })).collect::<Vec<_>>(),
        })
    }

    pub(super) fn validate(&self, channel: &ChannelDefinition) -> Result<(), ValidationError> {
        let invalid = || {
            ValidationError::new(
                "/secrets/profile_revisions",
                "invalid retained profile revision history",
            )
        };
        if channel.provider.slug.as_str() != "cma_codex"
            || !channel.provider.target.is_profile()
            || channel.secrets.provider_credential.is_none()
            || self.current_revision == 0
            || u64::try_from(self.retained.len())
                .ok()
                .and_then(|len| len.checked_add(1))
                != Some(self.current_revision)
            || (!self.baseline_complete && self.current_revision != 1)
            || (self.last_publication.is_some() != (self.current_revision > 1))
        {
            return Err(invalid());
        }
        if let Some(published) = &self.last_publication {
            if published.expected_version == 0 {
                return Err(invalid());
            }
            validate_hash(
                &published.request_fingerprint,
                "/secrets/profile_revisions/publication",
            )?;
        }
        let mut bindings = BTreeSet::from([channel.provider.binding_id.as_str()]);
        for (index, revision) in self.retained.iter().enumerate() {
            if revision.revision != index as u64 + 1
                || revision.provider.slug != channel.provider.slug
                || !revision.provider.target.is_profile()
                || !bindings.insert(revision.provider.binding_id.as_str())
                || revision.credential.as_str().is_empty()
            {
                return Err(invalid());
            }
            validate_hash(
                &revision.profile_digest,
                "/secrets/profile_revisions/profile_digest",
            )?;
            let profile: EventWorkerProfile =
                serde_json::from_value(revision.profile.clone()).map_err(|_| invalid())?;
            profile.validate()?;
            if profile.digest() != revision.profile_digest {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

/// Credentials are borrowed only after the caller has established the current
/// channel and the exact durable assignment.
pub struct ProfileRevisionView<'a> {
    pub revision: u64,
    pub profile_digest: &'a str,
    pub provider: &'a ProviderBinding,
    pub profile: EventWorkerProfile,
    pub credential: &'a Secret,
}

impl ChannelDefinition {
    pub fn profile_revision(&self, revision: u64) -> Option<ProfileRevisionView<'_>> {
        let history = self.secrets.profile_revisions.as_ref()?;
        if revision == history.current_revision {
            return Some(ProfileRevisionView {
                revision,
                profile_digest: &self.profile_digest,
                provider: &self.provider,
                profile: self.profile.clone(),
                credential: self.secrets.provider_credential.as_ref()?,
            });
        }
        let retained = history
            .retained
            .iter()
            .find(|value| value.revision == revision)?;
        Some(ProfileRevisionView {
            revision,
            profile_digest: &retained.profile_digest,
            provider: &retained.provider,
            profile: serde_json::from_value(retained.profile.clone()).ok()?,
            credential: &retained.credential,
        })
    }
}
