use layer2_core::sandbox::{WarmSandbox, WarmSandboxState};
use layer3_adapter::{AdapterError, SandboxAdapter};

pub(in crate::sandbox) async fn retire_environment(
    adapter: &dyn SandboxAdapter,
    record: &WarmSandbox,
) -> layer3_adapter::Result<WarmSandbox> {
    record
        .validate()
        .map_err(|error| AdapterError::Rejected(error.to_string()))?;
    if record.state == WarmSandboxState::Retired {
        return Ok(record.clone());
    }
    if record.state != WarmSandboxState::Retiring {
        return Err(AdapterError::Rejected(
            "legacy cleanup requires durable retirement".into(),
        ));
    }
    for observation in adapter.runtime_inventory().await? {
        if observation.sandbox_id != record.id.as_str() {
            continue;
        }
        if observation.generation != 1 {
            return Err(AdapterError::Rejected(
                "legacy cleanup runtime generation changed".into(),
            ));
        }
        adapter.reclaim_runtime(&observation).await?;
    }
    if adapter
        .runtime_inventory()
        .await?
        .iter()
        .any(|runtime| runtime.sandbox_id == record.id.as_str())
    {
        return Err(AdapterError::Unavailable(
            "legacy runtime cleanup is not complete".into(),
        ));
    }
    let mut next = record.clone();
    if let Some(home) = &mut next.home {
        crate::sandbox::home::release_home(adapter, home).await?;
    }
    next.state = WarmSandboxState::Retired;
    Ok(next)
}
