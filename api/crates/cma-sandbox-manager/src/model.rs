use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DOCUMENT_VERSION: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

/// The immutable environment request. Runtime bindings and ownership are
/// service-generated; callers cannot inject either through the request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSandbox {
    pub name: String,
    pub image: String,
    /// Empty uses the image's own command. No supervisor or Agent is injected.
    #[serde(default)]
    pub entrypoint: Vec<String>,
    pub resources: Resources,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub client_ref: Option<String>,
}

impl CreateSandbox {
    pub fn validate(&self) -> Result<(), ModelError> {
        if !bounded_text(&self.name, 128)
            || self
                .client_ref
                .as_ref()
                .is_some_and(|s| !bounded_text(s, 256))
        {
            return Err(ModelError::Invalid("invalid name or client_ref"));
        }
        // Accept user-selected OCI images, including tags. The provider records
        // the actual resolved image on the runtime binding before declaring it
        // ready; image contents do not define the service's domain model.
        if self.image.is_empty()
            || self.image.len() > 1024
            || !self
                .image
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"./_-:@".contains(&b))
            || self.image.contains("://")
        {
            return Err(ModelError::Invalid("invalid container image reference"));
        }
        if let Some((_, digest)) = self.image.split_once('@') {
            if !digest.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Err(ModelError::Invalid("invalid image digest"));
            }
        }
        if self.entrypoint.len() > 128
            || self
                .entrypoint
                .iter()
                .any(|arg| arg.len() > 8192 || arg.contains('\0'))
            || self.entrypoint.first().is_some_and(String::is_empty)
            || self.entrypoint.iter().map(String::len).sum::<usize>() > 65_536
        {
            return Err(ModelError::Invalid("invalid container command"));
        }
        if !(100..=64_000).contains(&self.resources.cpu_millis)
            || !(128..=262_144).contains(&self.resources.memory_mib)
        {
            return Err(ModelError::Invalid("resources exceed sandbox limits"));
        }
        if self.env.len() > 128
            || self.env.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 128
                    || !key.bytes().enumerate().all(|(i, b)| {
                        b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                    })
                    || value.len() > 8192
                    || value.contains('\0')
            })
            || self
                .env
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
                > 65_536
        {
            return Err(ModelError::Invalid("invalid environment variable"));
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> Result<String, ModelError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ModelError::Invalid("invalid request"))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

pub fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Accepted,
    /// Persisted before the single provider create request. Recovery observes
    /// this dispatch; absence from a list never authorizes a second create.
    Dispatching,
    Starting,
    Ready,
    Terminating,
    Terminated,
    Failed,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Dispatching => "dispatching",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Terminating => "terminating",
            Self::Terminated => "terminated",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBinding {
    pub provider_id: String,
    /// Captured immutable provider/Kubernetes identity, required for teardown.
    pub provider_uid: String,
    pub generation: i64,
    /// Content identity observed from the provider, including for tagged images.
    pub image_digest: String,
}

/// One service-owned aggregate. Creation idempotency and lifecycle changes use
/// single-document majority writes, without a cross-service transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    #[serde(rename = "_id")]
    pub id: String,
    pub schema_version: i32,
    pub owner: String,
    pub request_key_hash: String,
    pub request_sha256: String,
    pub spec: CreateSandbox,
    pub phase: Phase,
    pub revision: i64,
    pub generation: i64,
    pub dispatch_id: Option<String>,
    pub runtime: Option<RuntimeBinding>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub error: Option<String>,
}

impl Sandbox {
    pub fn admit(
        owner: &str,
        key: &str,
        spec: CreateSandbox,
        now: i64,
    ) -> Result<Self, ModelError> {
        if !bounded_text(owner, 512) || !bounded_text(key, 256) || now < 0 {
            return Err(ModelError::Invalid(
                "invalid owner, idempotency key, or timestamp",
            ));
        }
        let request_sha256 = spec.fingerprint()?;
        Ok(Self {
            id: format!("sbx_{}", uuid::Uuid::new_v4().simple()),
            schema_version: DOCUMENT_VERSION,
            owner: owner.to_owned(),
            request_key_hash: format!("{:x}", Sha256::digest(key.as_bytes())),
            request_sha256,
            spec,
            phase: Phase::Accepted,
            revision: 1,
            generation: 1,
            dispatch_id: None,
            runtime: None,
            created_at_ms: now,
            updated_at_ms: now,
            error: None,
        })
    }

    pub fn apply(&self, change: Change, now: i64) -> Result<Self, ModelError> {
        if self.schema_version != DOCUMENT_VERSION || now < self.updated_at_ms {
            return Err(ModelError::Invalid(
                "unsupported schema or regressing timestamp",
            ));
        }
        let mut next = self.clone();
        match change {
            Change::Dispatch { id } if self.phase == Phase::Accepted && bounded_text(&id, 128) => {
                next.phase = Phase::Dispatching;
                next.dispatch_id = Some(id);
            }
            Change::Bind {
                dispatch_id,
                runtime,
            } if self.phase == Phase::Dispatching
                && self.dispatch_id.as_deref() == Some(dispatch_id.as_str())
                && runtime.generation == self.generation
                && bounded_text(&runtime.provider_id, 256)
                && bounded_text(&runtime.provider_uid, 256)
                && runtime
                    .image_digest
                    .strip_prefix("sha256:")
                    .is_some_and(|hash| {
                        hash.len() == 64
                            && hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    }) =>
            {
                next.runtime = Some(runtime);
                next.phase = Phase::Starting;
                next.error = None;
            }
            Change::Ready { provider_uid }
                if self.phase == Phase::Starting
                    && self
                        .runtime
                        .as_ref()
                        .is_some_and(|r| r.provider_uid == provider_uid) =>
            {
                next.phase = Phase::Ready;
            }
            Change::Terminate if self.phase == Phase::Accepted => {
                // No dispatch has been admitted, so there can be no provider effect.
                next.phase = Phase::Terminated;
            }
            Change::Terminate
                if matches!(self.phase, Phase::Starting | Phase::Ready | Phase::Failed) =>
            {
                next.phase = if self.runtime.is_some() {
                    Phase::Terminating
                } else {
                    Phase::Terminated
                };
            }
            Change::Terminated { provider_uid }
                if self.phase == Phase::Terminating
                    && self
                        .runtime
                        .as_ref()
                        .is_some_and(|r| r.provider_uid == provider_uid) =>
            {
                next.phase = Phase::Terminated;
            }
            Change::Failed { detail }
                if matches!(self.phase, Phase::Accepted | Phase::Starting)
                    && bounded_text(&detail, 1024) =>
            {
                next.phase = Phase::Failed;
                next.error = Some(detail);
            }
            _ => return Err(ModelError::Conflict),
        }
        next.revision = self.revision.checked_add(1).ok_or(ModelError::Conflict)?;
        next.updated_at_ms = now;
        Ok(next)
    }
}

#[derive(Debug, Clone)]
pub enum Change {
    Dispatch {
        id: String,
    },
    Bind {
        dispatch_id: String,
        runtime: RuntimeBinding,
    },
    Ready {
        provider_uid: String,
    },
    Terminate,
    Terminated {
        provider_uid: String,
    },
    Failed {
        detail: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("sandbox state or runtime identity does not permit this operation")]
    Conflict,
}
