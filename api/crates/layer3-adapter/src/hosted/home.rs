//! Mount the initialized home layout without giving the runtime allocation authority.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

use crate::provider::{ProviderPersistentVolumeClaim, ProviderSandboxRequest, ProviderVolume};
use crate::{AdapterError, SandboxSpec};

pub(super) const TRANSFER_ROOT: &str = "/var/lib/cma-transfer";

pub(super) fn render(
    request: &mut ProviderSandboxRequest,
    spec: &SandboxSpec,
) -> crate::Result<()> {
    let Some(home) = &spec.home_volume else {
        return Ok(());
    };
    home.validate_for(&spec.sandbox_id)?;
    let encoded = serde_json::to_vec(home)
        .map_err(|_| AdapterError::Rejected("durable home identity cannot be encoded".into()))?;
    request.metadata.insert(
        "cma/home-identity".into(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(encoded)),
    );
    for (name, mount_path, sub_path) in [
        ("cma-home", spec.home_directory.as_str(), "home"),
        ("cma-state", "/var/lib/cma-runtime", "state"),
        ("cma-transfer", TRANSFER_ROOT, "transfer"),
    ] {
        request.volumes.push(ProviderVolume {
            name: name.into(),
            pvc: ProviderPersistentVolumeClaim {
                claim_name: home.receipt.storage_reference.clone(),
                create_if_not_exists: false,
                delete_on_sandbox_termination: false,
            },
            mount_path: mount_path.into(),
            read_only: false,
            sub_path: Some(sub_path.into()),
        });
    }
    Ok(())
}
