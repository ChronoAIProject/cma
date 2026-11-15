use crate::{provider::ProviderSandboxRequest, AdapterError, Result};

pub(super) fn allocation_identity(request: &ProviderSandboxRequest) -> Result<String> {
    let sandbox = request
        .metadata
        .get("cma/sandbox")
        .ok_or_else(|| AdapterError::Rejected("effect has no durable sandbox identity".into()))?;
    let generation = request
        .metadata
        .get("cma/runtime-generation")
        .ok_or_else(|| AdapterError::Rejected("effect has no durable runtime generation".into()))?;
    Ok(format!("sandbox:{sandbox}:generation:{generation}"))
}
