use super::{digest, observe, Identity, Limits, Result, MAX_OUTPUT};
use crate::store::ledger_census::files;
use mongodb::{options::ClientOptions, Client};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, os::unix::fs::MetadataExt, path::Path, time::Duration};

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|arg| arg.starts_with("--home-backup-catalog-export"))
}

pub(crate) async fn command(arguments: &[String]) -> Result<String> {
    let [flag, output] = arguments else {
        return Err("catalog_export_usage");
    };
    if flag != "--home-backup-catalog-export" {
        return Err("catalog_export_usage");
    }
    let output = Path::new(output);
    require_destination(output)?;
    let identity = Identity {
        source_revision_claim: std::env::var("CHRONO_HOME_BACKUP_EXPORT_SOURCE_SHA")
            .map_err(|_| "catalog_export_identity_required")?,
        executable_sha256: executable_digest()?,
    };
    identity.validate()?;
    tokio::time::timeout(Duration::from_secs(75), async {
        let uri = std::env::var("CHRONO_MONGODB_URI")
            .map_err(|_| "catalog_export_configuration_required")?;
        let name = std::env::var("CHRONO_MONGODB_DATABASE")
            .map_err(|_| "catalog_export_configuration_required")?;
        let mut options = ClientOptions::parse(uri)
            .await
            .map_err(|_| "catalog_export_connection_failed")?;
        crate::store::transaction_barrier::bind(&mut options, &name);
        options.server_selection_timeout = Some(Duration::from_secs(5));
        options.connect_timeout = Some(Duration::from_secs(5));
        options.retry_reads = Some(false);
        options.retry_writes = Some(false);
        options.app_name = Some("cma-home-backup-catalog-export".into());
        let client =
            Client::with_options(options).map_err(|_| "catalog_export_connection_failed")?;
        let bytes = observe(
            &client,
            &client.database(&name),
            identity,
            Limits::default(),
            #[cfg(test)]
            &(),
        )
        .await?;
        publish(output, &bytes)
    })
    .await
    .map_err(|_| "catalog_export_timeout")?
}

fn executable_digest() -> Result<String> {
    let path = std::env::current_exe().map_err(|_| "catalog_export_executable_unavailable")?;
    let mut file = fs::File::open(path).map_err(|_| "catalog_export_executable_unavailable")?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "catalog_export_executable_unavailable")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > 512 * 1024 * 1024 {
            return Err("catalog_export_executable_unavailable");
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn require_destination(path: &Path) -> Result<()> {
    files::require_parent(path).map_err(|_| "catalog_export_private_path_required")?;
    let parent = path
        .parent()
        .ok_or("catalog_export_private_path_required")?;
    if fs::canonicalize(parent).ok().as_deref() != Some(parent)
        || !matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Err("catalog_export_private_new_file_required");
    }
    Ok(())
}

pub(in crate::store::home_backups) fn publish(path: &Path, bytes: &[u8]) -> Result<String> {
    require_destination(path)?;
    let envelope: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "catalog_export_output_invalid")?;
    let payload_json = envelope["payload_json"]
        .as_str()
        .ok_or("catalog_export_output_invalid")?;
    if bytes.len() > MAX_OUTPUT || envelope["payload_sha256"] != digest(payload_json.as_bytes()) {
        return Err("catalog_export_output_invalid");
    }
    let payload: serde_json::Value =
        serde_json::from_str(payload_json).map_err(|_| "catalog_export_output_invalid")?;
    files::publish(path, bytes).map_err(|_| "catalog_export_publication_failed")?;
    let before = fs::symlink_metadata(path).map_err(|_| "catalog_export_publication_failed")?;
    let file = fs::File::open(path).map_err(|_| "catalog_export_publication_failed")?;
    let opened = file
        .metadata()
        .map_err(|_| "catalog_export_publication_failed")?;
    if !before.is_file()
        || before.mode() & 0o777 != 0o600
        || before.nlink() != 1
        || (before.dev(), before.ino()) != (opened.dev(), opened.ino())
    {
        return Err("catalog_export_publication_failed");
    }
    let mut retained = Vec::new();
    file.take(MAX_OUTPUT as u64 + 1)
        .read_to_end(&mut retained)
        .map_err(|_| "catalog_export_publication_failed")?;
    if retained != bytes {
        return Err("catalog_export_publication_failed");
    }
    let counts: serde_json::Map<String, serde_json::Value> = super::COLLECTIONS
        .iter()
        .map(|name| {
            (
                (*name).into(),
                payload["collections"][*name]["count"].clone(),
            )
        })
        .collect();
    Ok(json!({
        "format": "home-backup-catalog-receipt-v1", "artifact_sha256": digest(bytes),
        "payload_sha256": envelope["payload_sha256"], "run_sha256": digest(payload["run_id"].as_str().ok_or("catalog_export_output_invalid")?.as_bytes()),
        "source_revision_claim": payload["identity"]["source_revision_claim"],
        "executable_sha256": payload["identity"]["executable_sha256"], "counts": counts,
    }).to_string())
}
