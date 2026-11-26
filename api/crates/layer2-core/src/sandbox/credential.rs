use serde::{Deserialize, Serialize};

use crate::ids::{ServiceId, SubjectId};

/// Who owns credential rotation and provider revocation. Supplied values are
/// delivered from encrypted custody; their issuer retains those responsibilities.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxCredentialSource {
    #[default]
    Managed,
    ManagedConnection {
        initial: InitialServiceConnection,
    },
    Provided {
        key_sha256: String,
    },
    /// Stable supplied-key authority; each custody generation pins its own hash.
    Binding {
        binding_id: String,
        /// The immutable destination allowed to receive this supplied credential.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_endpoint: Option<String>,
    },
}

impl SandboxCredentialSource {
    pub fn is_managed(&self) -> bool {
        matches!(self, Self::Managed | Self::ManagedConnection { .. })
    }

    pub fn is_default_managed(&self) -> bool {
        matches!(self, Self::Managed)
    }

    pub fn initial_connection(&self) -> Option<&InitialServiceConnection> {
        match self {
            Self::ManagedConnection { initial } => Some(initial),
            _ => None,
        }
    }

    pub fn validate(&self) -> bool {
        match self {
            Self::Managed | Self::ManagedConnection { .. } => true,
            Self::Binding {
                binding_id,
                tool_endpoint,
            } => {
                super::binding::valid_binding_id(binding_id)
                    && tool_endpoint
                        .as_deref()
                        .is_none_or(super::binding::valid_tool_endpoint)
            }
            Self::Provided { key_sha256 } => {
                key_sha256.len() == 64
                    && key_sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            }
        }
    }

    pub fn accepts(&self, authority: &SandboxCredentialAuthority) -> bool {
        match self {
            Self::Managed | Self::ManagedConnection { .. } => !authority.expires_at.is_empty(),
            Self::Binding { .. } => {
                self.validate()
                    && authority
                        .provider_key_id
                        .strip_prefix("provided-")
                        .is_some_and(|hash| {
                            hash.len() == 64
                                && hash
                                    .bytes()
                                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                        })
                    && authority.provider_state_version == 1
                    && authority.expires_at.is_empty()
            }
            Self::Provided { key_sha256 } => {
                self.validate()
                    && authority.provider_key_id == format!("provided-{key_sha256}")
                    && authority.provider_state_version == 1
                    && authority.expires_at.is_empty()
            }
        }
    }
}

/// Explicit owner authorization added after the immutable creation scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxServiceGrant {
    pub services: Vec<ServiceId>,
    pub authorized_by: SubjectId,
    pub authorized_at: String,
    /// Credentials at or below this reservation predate the expanded scope.
    pub credential_generation_floor: u64,
}

/// An owner-selected connection for one service in a Sandbox scope revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConnectionSelection {
    pub service: ServiceId,
    pub connection_id: String,
}

impl ServiceConnectionSelection {
    pub fn matches_scope(&self, services: &[ServiceId]) -> bool {
        services.contains(&self.service)
            && !self.connection_id.is_empty()
            && self.connection_id.len() <= 128
            && self
                .connection_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialServiceConnection {
    pub selection: ServiceConnectionSelection,
    pub requirement: RepositoryCredentialRequirement,
    pub repository: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryCredentialRequirement {
    PublicRepositoryWrite,
    PrivateRepositoryRead,
}

/// The latest exact owner-approved scope, separate from immutable creation scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxServiceScopeRevision {
    pub revision: u64,
    pub services: Vec<ServiceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ServiceConnectionSelection>,
    pub authorized_by: SubjectId,
    pub authorized_at: String,
    /// Credentials at or below this reservation predate this exact scope.
    pub credential_generation_floor: u64,
    /// Runtime whose original startup environment is retained during this revision.
    pub runtime: super::RuntimeBinding,
    /// Startup service names are immutable for an existing runtime even while its key rotates.
    pub runtime_services: Vec<ServiceId>,
}

/// Non-secret evidence for one sandbox credential. For supplied credentials the
/// key id is a hash receipt, state version is one and expiry is unknown (empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCredentialAuthority {
    pub generation: u64,
    pub provider_key_id: String,
    pub provider_state_version: i64,
    pub issued_at: String,
    pub expires_at: String,
    pub grant_fingerprint: String,
}

/// A provider key which must remain visible until revocation succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRevokeDebt {
    pub provider_key_id: String,
    pub queued_at: String,
}

/// Provider sign-in state for profiles that use the owner's own model account.
/// CMA records only the state; credential bytes stay inside the sandbox home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxSignInState {
    NotRequired,
    Required,
    Complete,
}

/// Durable, non-secret sandbox credential custody metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCredentialState {
    #[serde(
        default,
        skip_serializing_if = "SandboxCredentialSource::is_default_managed"
    )]
    pub source: SandboxCredentialSource,
    #[serde(default)]
    pub reaches: Vec<ServiceId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_grants: Vec<SandboxServiceGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_scope: Option<SandboxServiceScopeRevision>,
    /// Highest custody generation reserved for this sandbox.
    pub key_generation: u64,
    pub delivered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<SandboxCredentialAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<SandboxCredentialAuthority>,
    #[serde(default)]
    pub revoke_debt: Vec<SandboxRevokeDebt>,
    pub sign_in: SandboxSignInState,
}

impl SandboxCredentialState {
    pub fn selected_connection(&self) -> Option<&ServiceConnectionSelection> {
        match &self.service_scope {
            Some(scope) => scope.connection.as_ref(),
            None => self
                .source
                .initial_connection()
                .map(|initial| &initial.selection),
        }
    }

    pub fn selected_repository_connection(&self) -> Option<&InitialServiceConnection> {
        let initial = self.source.initial_connection()?;
        (self.selected_connection() == Some(&initial.selection)).then_some(initial)
    }

    pub fn service_access_generation_floor(&self) -> u64 {
        if let Some(scope) = &self.service_scope {
            return scope.credential_generation_floor;
        }
        self.service_grants
            .last()
            .map_or(0, |grant| grant.credential_generation_floor)
    }

    pub fn effective_reaches(&self) -> Vec<ServiceId> {
        if let Some(scope) = &self.service_scope {
            return scope.services.clone();
        }
        self.reaches
            .iter()
            .chain(self.service_grants.iter().flat_map(|grant| &grant.services))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Recover the environment originally rendered for this runtime generation.
    /// A replaced process can change its boot identity without changing that environment.
    pub fn runtime_reaches(&self, runtime: &super::RuntimeBinding) -> Vec<ServiceId> {
        if let Some(scope) = self.service_scope.as_ref().filter(|scope| {
            scope.runtime.runtime_id == runtime.runtime_id
                && scope.runtime.runtime_version == runtime.runtime_version
                && scope.runtime.provision_generation == runtime.provision_generation
        }) {
            return scope.runtime_services.clone();
        }
        self.effective_reaches()
    }

    pub fn validate(&self) -> bool {
        let authority_valid = |authority: &SandboxCredentialAuthority| {
            self.source.accepts(authority)
                && authority.generation > 0
                && authority.generation <= self.key_generation
                && !authority.provider_key_id.trim().is_empty()
                && authority.provider_state_version > 0
                && authority.grant_fingerprint.len() == 64
                && authority
                    .grant_fingerprint
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        };
        self.source.validate()
            && self
                .source
                .initial_connection()
                .is_none_or(|initial| initial.selection.matches_scope(&self.reaches))
            && (self.source.is_managed()
                || (self.service_grants.is_empty() && self.service_scope.is_none()))
            && self.current.as_ref().is_none_or(authority_valid)
            && self.pending.as_ref().is_none_or(authority_valid)
            && self.delivered == self.current.is_some()
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.generation == self.key_generation)
            && self.revoke_debt.iter().all(|debt| {
                !debt.provider_key_id.trim().is_empty()
                    && self
                        .current
                        .as_ref()
                        .is_none_or(|current| current.provider_key_id != debt.provider_key_id)
                    && self
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.provider_key_id != debt.provider_key_id)
            })
    }

    pub fn next_generation(&self) -> Option<u64> {
        self.key_generation.checked_add(1)
    }

    pub fn queue_revoke(&mut self, provider_key_id: String, queued_at: String) {
        if !self
            .revoke_debt
            .iter()
            .any(|debt| debt.provider_key_id == provider_key_id)
        {
            self.revoke_debt.push(SandboxRevokeDebt {
                provider_key_id,
                queued_at,
            });
        }
    }
}
