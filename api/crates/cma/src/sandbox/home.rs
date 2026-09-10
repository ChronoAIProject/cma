//! Each returned home change must commit before another external storage effect.

use layer2_core::sandbox::SandboxHome;
use layer2_core::SandboxId;
use layer3_adapter::{AdapterError, HomeVolumePolicy, SandboxAdapter};

pub(super) async fn next_home(
    adapter: &dyn SandboxAdapter,
    id: &SandboxId,
    current: Option<&SandboxHome>,
    policy: Option<&HomeVolumePolicy>,
) -> Result<Option<SandboxHome>, AdapterError> {
    let Some(home) = current else {
        return policy
            .map(|policy| {
                SandboxHome::reserve(id, policy.clone())
                    .map_err(|error| AdapterError::Rejected(error.to_string()))
            })
            .transpose();
    };
    home.validate(id)
        .map_err(|error| AdapterError::Rejected(error.to_string()))?;
    if home.released {
        return Err(AdapterError::Rejected(
            "released home cannot be provisioned again".into(),
        ));
    }
    let store = adapter.home_volumes().ok_or_else(|| {
        AdapterError::Unsupported("durable home storage is not configured".into())
    })?;
    if let Some(attachment) = home.attachment() {
        store.verify(&attachment).await?;
        return Ok(None);
    }
    let receipt = store.prepare(&home.request).await?;
    let mut next = home.clone();
    next.bind(receipt)
        .map_err(|error| AdapterError::Rejected(error.to_string()))?;
    Ok(Some(next))
}

pub(super) async fn release_home(
    adapter: &dyn SandboxAdapter,
    home: &mut SandboxHome,
) -> Result<(), AdapterError> {
    if !home.released {
        let store = adapter.home_volumes().ok_or_else(|| {
            AdapterError::Unsupported("durable home storage is not configured".into())
        })?;
        if let Some(attachment) = home.attachment() {
            store.remove_captures(&attachment).await?;
        }
        store.remove(&home.request, home.receipt.as_ref()).await?;
        home.released = true;
    }
    Ok(())
}
