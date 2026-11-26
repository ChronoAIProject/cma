use super::{observe, CompleteInventory, Limits, Result};
use crate::store::ledger_census::files;
use mongodb::{options::ClientOptions, Client};
use std::{fs, io::Read, os::unix::fs::MetadataExt, path::Path, time::Duration};

const FLAG: &str = "--turn-budget-inventory";
const OUTPUT: &str = "turn_inventory_private_new_file_required";

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| argument.starts_with(FLAG))
}

fn destination(arguments: &[String]) -> Result<&Path> {
    match arguments {
        [flag, output] if flag == FLAG => {
            let path = Path::new(output);
            require_destination(path)?;
            Ok(path)
        }
        _ => Err("turn_inventory_usage"),
    }
}

pub(crate) async fn command(arguments: &[String]) -> Result<String> {
    let path = destination(arguments)?;
    tokio::time::timeout(Duration::from_secs(75), async {
        let uri = std::env::var("CHRONO_MONGODB_URI")
            .map_err(|_| "turn_inventory_configuration_required")?;
        let name = std::env::var("CHRONO_MONGODB_DATABASE")
            .map_err(|_| "turn_inventory_configuration_required")?;
        let mut options = ClientOptions::parse(uri)
            .await
            .map_err(|_| "turn_inventory_connection_failed")?;
        options.server_selection_timeout = Some(Duration::from_secs(5));
        options.connect_timeout = Some(Duration::from_secs(5));
        options.retry_reads = Some(false);
        options.retry_writes = Some(false);
        options.app_name = Some("cma-turn-budget-inventory".into());
        let client =
            Client::with_options(options).map_err(|_| "turn_inventory_connection_failed")?;
        let report = observe(
            &client,
            &client.database(&name),
            Limits::default(),
            #[cfg(test)]
            &(),
        )
        .await?;
        publish(path, &report)
    })
    .await
    .map_err(|_| "turn_inventory_timeout")?
}

fn require_destination(path: &Path) -> Result<()> {
    files::require_parent(path).map_err(|_| OUTPUT)?;
    let parent = path.parent().ok_or(OUTPUT)?;
    if fs::canonicalize(parent).ok().as_deref() != Some(parent)
        || !matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Err(OUTPUT);
    }
    Ok(())
}

pub(super) fn publish(path: &Path, report: &CompleteInventory) -> Result<String> {
    require_destination(path)?;
    files::publish(path, &report.bytes).map_err(|_| OUTPUT)?;
    let before = fs::symlink_metadata(path).map_err(|_| OUTPUT)?;
    let file = fs::File::open(path).map_err(|_| OUTPUT)?;
    let opened = file.metadata().map_err(|_| OUTPUT)?;
    if !before.is_file()
        || before.mode() & 0o777 != 0o600
        || before.nlink() != 1
        || (before.dev(), before.ino()) != (opened.dev(), opened.ino())
    {
        return Err(OUTPUT);
    }
    let mut bytes = Vec::new();
    file.take(Limits::default().output_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OUTPUT)?;
    if bytes != report.bytes {
        return Err(OUTPUT);
    }
    Ok(report.receipt())
}
