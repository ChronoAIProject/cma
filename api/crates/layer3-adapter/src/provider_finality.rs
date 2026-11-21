//! Provider receipts bind cleanup to the runtime incarnation adopted before transport binding.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AdapterError, HomeVolumeScope, Result, RuntimeHandle};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProviderIncarnation {
    pub(crate) scope: HomeVolumeScope,
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) uid: String,
    pub(crate) generation: i64,
    pub(crate) binding_resource_version: String,
    pub(crate) identity_sha256: String,
}

impl RuntimeProviderIncarnation {
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        if !identifier(&self.namespace)
            || !identifier(&self.name)
            || !identifier(&self.uid)
            || self.generation <= 0
            || !identifier(&self.binding_resource_version)
            || !digest(&self.identity_sha256)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProvisionReceipt {
    pub(crate) operation_id: String,
    pub(crate) request_sha256: String,
    pub(crate) provider_response_sha256: String,
    pub(crate) handle: RuntimeHandle,
    pub(crate) incarnation: RuntimeProviderIncarnation,
    pub(crate) pre_dispatch_list_version: String,
}

impl RuntimeProvisionReceipt {
    pub fn handle(&self) -> &RuntimeHandle {
        &self.handle
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn incarnation(&self) -> &RuntimeProviderIncarnation {
        &self.incarnation
    }
    pub fn validate(&self) -> Result<()> {
        self.incarnation.validate()?;
        if !identifier(&self.operation_id)
            || !identifier(&self.pre_dispatch_list_version)
            || !digest(&self.request_sha256)
            || !digest(&self.provider_response_sha256)
            || !(self.handle.0 == self.incarnation.name
                || crate::agent_sandbox::parse_handle(&self.handle).is_ok_and(|(name, uid)| {
                    name == self.incarnation.name && uid == self.incarnation.uid
                }))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFinalityReceipt {
    pub(crate) original: RuntimeProvisionReceipt,
    pub(crate) delete_operation_id: String,
    pub(crate) delete_resource_version: String,
    pub(crate) delete_response_sha256: String,
    pub(crate) absent_runtime_list_version: String,
    pub(crate) absent_owned_pods_list_version: String,
}

impl RuntimeFinalityReceipt {
    pub fn original(&self) -> &RuntimeProvisionReceipt {
        &self.original
    }
    pub fn delete_operation_id(&self) -> &str {
        &self.delete_operation_id
    }
    pub fn validate(&self) -> Result<()> {
        self.original.validate()?;
        if !identifier(&self.delete_operation_id)
            || !identifier(&self.delete_resource_version)
            || !digest(&self.delete_response_sha256)
            || !identifier(&self.absent_runtime_list_version)
            || !identifier(&self.absent_owned_pods_list_version)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub fn provider_request_digest(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    let mut digest = Sha256::new();
    digest.update(b"cma-provider-effect-v1\0");
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}
pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn invalid() -> AdapterError {
    AdapterError::Rejected("provider finality receipt has invalid bound identity".into())
}
