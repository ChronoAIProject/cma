//! Retained filesystem authority for deterministic runtime replacement tests.

use std::collections::BTreeMap;

use super::authority::ResourceKey;
use super::{provider_request_identity, MemoryRuntime};
use crate::wire::ResponseFrame;
use crate::{AdapterError, RuntimeHandle};

fn identity(runtime: &MemoryRuntime) -> Option<&str> {
    runtime
        .request
        .env
        .get("CHRONO_RUNTIME_HOME_IDENTITY")
        .filter(|identity| !identity.is_empty())
        .map(String::as_str)
}

pub(super) fn retain(
    mut runtime: MemoryRuntime,
    homes: &mut BTreeMap<String, MemoryRuntime>,
) -> crate::Result<()> {
    let Some(home) = identity(&runtime).map(str::to_owned) else {
        return Ok(());
    };
    let owner = provider_request_identity(&runtime.request)?;
    if let Some(saved) = homes.get(&home) {
        let previous = provider_request_identity(&saved.request)?;
        if previous.sandbox_id != owner.sandbox_id || previous.generation > owner.generation {
            return Err(AdapterError::Rejected(
                "retained home authority was superseded".into(),
            ));
        }
    }
    for (slot, agent) in &runtime.agents {
        runtime
            .restored_conversations
            .insert(slot.clone(), agent.conversation_id.clone());
    }
    runtime.agents.clear();
    runtime.credential_delivered = false;
    runtime.held_checkpoint = None;
    runtime.resumed_checkpoint_tokens.clear();
    homes.insert(home, runtime);
    Ok(())
}

pub(super) fn attach(
    runtime: &mut MemoryRuntime,
    live: &BTreeMap<RuntimeHandle, MemoryRuntime>,
    homes: &BTreeMap<String, MemoryRuntime>,
) -> crate::Result<()> {
    let Some(home) = identity(runtime) else {
        return Ok(());
    };
    if live.values().any(|other| identity(other) == Some(home)) {
        return Err(AdapterError::Rejected(
            "retained home is already mounted by a runtime".into(),
        ));
    }
    let Some(saved) = homes.get(home) else {
        return Ok(());
    };
    let previous = provider_request_identity(&saved.request)?;
    let next = provider_request_identity(&runtime.request)?;
    if previous.sandbox_id != next.sandbox_id || previous.generation > next.generation {
        return Err(AdapterError::Rejected(
            "retained home belongs to another runtime authority".into(),
        ));
    }
    let request = runtime.request.clone();
    let boot_id = runtime.boot_id.clone();
    *runtime = saved.clone();
    runtime.request = request;
    runtime.boot_id = boot_id;
    if next.generation > previous.generation {
        runtime
            .authorities
            .retain(|resource, _| !matches!(resource, ResourceKey::AgentProcess(..)));
        runtime.operations.retain(|_, operation| {
            !matches!(
                operation.response,
                ResponseFrame::OpenAgent(_) | ResponseFrame::CloseAgent(_)
            )
        });
    }
    Ok(())
}
