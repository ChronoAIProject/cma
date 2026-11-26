//! Durable ownership and archive completion for a mounted Sandbox home.

use layer3_adapter::{RestoreRequest, RuntimeStatusRequest};
use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::install_state::{InstallRecord, RecoveryState};

#[path = "persistent_home_io.rs"]
mod io;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    version: u8,
    sandbox_id: String,
    home_identity: String,
    runtime_generation: u64,
    restore: Option<RestoreMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreMarker {
    request_sha256: String,
    completed: bool,
}

impl Binding {
    pub(crate) fn validate_inventory(bytes: &[u8], config: &RuntimeConfig) -> Result<()> {
        Self::decode_inventory(bytes, config).map(|_| ())
    }

    pub(crate) fn decode_inventory(bytes: &[u8], config: &RuntimeConfig) -> Result<Self> {
        let binding: Self =
            serde_json::from_slice(bytes).map_err(|_| invalid("home binding is malformed"))?;
        binding.validate(config)?;
        if binding.runtime_generation != config.generation {
            return Err(invalid(
                "home binding generation does not match the inventory",
            ));
        }
        Ok(binding)
    }

    fn new(config: &RuntimeConfig) -> Result<Self> {
        let request = RuntimeStatusRequest {
            sandbox_id: config.sandbox_id.clone(),
            generation: config.generation,
            home_identity: config
                .home_identity
                .clone()
                .ok_or_else(|| invalid("home identity is absent"))?,
        };
        request
            .validate()
            .map_err(|_| invalid("home identity is invalid"))?;
        Ok(Self {
            version: 2,
            sandbox_id: request.sandbox_id,
            home_identity: request.home_identity,
            runtime_generation: config.generation,
            restore: None,
        })
    }

    fn validate(&self, config: &RuntimeConfig) -> Result<()> {
        let expected = Self::new(config)?;
        if self.version != expected.version
            || self.sandbox_id != expected.sandbox_id
            || self.home_identity != expected.home_identity
            || self.runtime_generation == 0
            || self.restore.as_ref().is_some_and(|restore| {
                restore.request_sha256.len() != 64
                    || !restore
                        .request_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(invalid(
                "persistent home belongs to another identity or has invalid state",
            ));
        }
        Ok(())
    }

    pub(crate) fn restore_completed(&self, state: &RecoveryState) -> Result<bool> {
        match (state, &self.restore) {
            (RecoveryState::Inherited(receipt), Some(marker))
                if receipt.restore_request_sha256 == marker.request_sha256 =>
            {
                Ok(marker.completed)
            }
            (_, None) => Ok(false),
            _ => Err(invalid(
                "home restore marker disagrees with installation authority",
            )),
        }
    }

    pub(crate) fn installation_ready(&self, record: Option<&InstallRecord>) -> Result<bool> {
        match (record, &self.restore) {
            (Some(InstallRecord::Completed { .. }), None) => Ok(true),
            (Some(InstallRecord::Inherited { receipt, .. }), Some(marker))
                if receipt.restore_request_sha256 == marker.request_sha256 =>
            {
                Ok(marker.completed)
            }
            (_, None) => Ok(false),
            _ => Err(invalid(
                "home restore marker disagrees with installation authority",
            )),
        }
    }
}

pub(crate) fn initialize(config: &RuntimeConfig) -> Result<Option<Binding>> {
    if config.home_identity.is_none() {
        return Ok(None);
    }
    let existing = io::read(config)?;
    let mut binding = match existing {
        Some(binding) => binding,
        None => {
            if crate::install_state::inspect(config)?.is_some() || !io::uninitialized(config)? {
                return Err(invalid(
                    "an existing runtime home has no durable ownership binding",
                ));
            }
            let binding = Binding::new(config)?;
            io::publish(config, &binding, true)?;
            binding
        }
    };
    binding.validate(config)?;
    if config.generation < binding.runtime_generation {
        return Err(invalid(
            "an older runtime generation cannot reclaim this home",
        ));
    }
    if config.generation > binding.runtime_generation {
        // Provider replacement has exclusive access to the same recorded disk.
        // Advance authority before journal recovery or credential delivery.
        binding.runtime_generation = config.generation;
        io::publish(config, &binding, false)?;
    }
    io::clean_temporary(config)?;
    Ok(Some(binding))
}

pub(crate) fn verify(config: &RuntimeConfig) -> Result<Option<Binding>> {
    if config.home_identity.is_none() {
        return Ok(None);
    }
    if !crate::paths::path_is_directory(&config.home) {
        return Err(invalid("persistent home directory is missing or replaced"));
    }
    let binding = io::read(config)?.ok_or_else(|| invalid("persistent home binding is missing"))?;
    binding.validate(config)?;
    if binding.runtime_generation != config.generation {
        return Err(invalid(
            "this runtime generation no longer owns the persistent home",
        ));
    }
    Ok(Some(binding))
}

/// Initial publication recovery must never reuse restored or advanced authority.
pub(crate) fn verify_initial(config: &RuntimeConfig) -> Result<()> {
    if config.generation != 1 || verify(config)?.as_ref() != Some(&Binding::new(config)?) {
        return Err(invalid("home is not an initial un-restored binding"));
    }
    Ok(())
}

/// Return true for an already completed exact restore. Never overwrite newer home contents.
pub(crate) fn begin_restore(config: &RuntimeConfig, request: &RestoreRequest) -> Result<bool> {
    let Some(mut binding) = verify(config)? else {
        return Ok(false);
    };
    let Some(InstallRecord::Inherited { receipt, .. }) = crate::install_state::inspect(config)?
    else {
        return Err(invalid(
            "persistent archive restore requires inherited installation authority",
        ));
    };
    let digest = request.computed_sha256();
    if !request.has_valid_reference() || receipt.restore_request_sha256 != digest {
        return Err(invalid(
            "archive restore does not match the persistent home authority",
        ));
    }
    if let Some(marker) = binding.restore {
        return if marker.request_sha256 == digest {
            Ok(marker.completed)
        } else {
            Err(invalid("persistent home has another restore identity"))
        };
    }
    binding.restore = Some(RestoreMarker {
        request_sha256: digest,
        completed: false,
    });
    io::publish(config, &binding, false)?;
    Ok(false)
}

pub(crate) fn complete_restore(config: &RuntimeConfig, request: &RestoreRequest) -> Result<()> {
    let Some(mut binding) = verify(config)? else {
        return Ok(());
    };
    let marker = binding
        .restore
        .as_mut()
        .ok_or_else(|| invalid("restore was not durably started"))?;
    if marker.request_sha256 != request.computed_sha256() {
        return Err(invalid("restore completion names another archive"));
    }
    marker.completed = true;
    io::publish(config, &binding, false)
}

fn invalid(detail: &str) -> RuntimeError {
    RuntimeError::rejected("persistent_home_invalid", detail)
}
