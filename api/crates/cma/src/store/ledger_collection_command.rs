//! Explicit read-only entry point. No server loop or executing collector is enabled.
use layer3_adapter::runtime_ledger_collection::{CollectionRequest, MAX_COLLECTION_REQUEST_BYTES};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    time::Duration,
};

use super::observe;

const UNAVAILABLE: &str = "bounded ledger collection command refused";
type Result<T> = std::result::Result<T, &'static str>;

pub(crate) async fn command(request: &Path, destination: &Path) -> Result<()> {
    let request = read_request(request)?;
    let parent = destination.parent().ok_or(UNAVAILABLE)?;
    let directory = std::fs::symlink_metadata(parent).map_err(|_| UNAVAILABLE)?;
    if !directory.is_dir()
        || std::fs::canonicalize(parent).map_err(|_| UNAVAILABLE)? != parent
        || !destination.is_absolute()
        || destination.file_name().is_none()
        || directory.permissions().mode() & 0o777 != 0o700
    {
        return Err(UNAVAILABLE);
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|_| UNAVAILABLE)?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| UNAVAILABLE)?;
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| UNAVAILABLE)?;
    let mode = std::env::var("CHRONO_MONGODB_AUTHORITY_MODE").ok();
    let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::parse(mode.as_deref())
        .map_err(|_| UNAVAILABLE)?;
    let observation = tokio::time::timeout(Duration::from_secs(10), async {
        let mut options = mongodb::options::ClientOptions::parse(uri)
            .await
            .map_err(|_| UNAVAILABLE)?;
        crate::store::transaction_barrier::bind(&mut options, &name);
        let client = mongodb::Client::with_options(options).map_err(|_| UNAVAILABLE)?;
        let database = client.database(&name);
        crate::store::isolation::verify_with_authority(&database, authority)
            .await
            .map_err(|_| UNAVAILABLE)?;
        observe(&client, &database, &request)
            .await
            .map_err(|_| UNAVAILABLE)
    })
    .await
    .map_err(|_| UNAVAILABLE)??;
    let bytes = serde_json::to_vec(&observation).map_err(|_| UNAVAILABLE)?;
    if output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .and_then(|()| std::fs::File::open(parent)?.sync_all())
        .is_err()
    {
        let _ = output.set_len(0);
        return Err(UNAVAILABLE);
    }
    println!(
        "{}",
        serde_json::json!({
            "format":"runtime-ledger-authority-summary-v1",
            "observation_sha256":format!("{:x}",Sha256::digest(&bytes)),
            "groups":observation.groups.len(),"dependencies_read":observation.dependencies_read,
            "source_bytes":observation.source_bytes,"apply_authority":false,
        })
    );
    Ok(())
}

fn read_request(path: &Path) -> Result<CollectionRequest> {
    if !path.is_absolute() || std::fs::canonicalize(path).map_err(|_| UNAVAILABLE)? != path {
        return Err(UNAVAILABLE);
    }
    let before = std::fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
    if !before.is_file()
        || before.permissions().mode() & 0o777 != 0o600
        || before.len() > MAX_COLLECTION_REQUEST_BYTES as u64
    {
        return Err(UNAVAILABLE);
    }
    let file = std::fs::File::open(path).map_err(|_| UNAVAILABLE)?;
    let opened = file.metadata().map_err(|_| UNAVAILABLE)?;
    if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
        return Err(UNAVAILABLE);
    }
    let mut bytes = Vec::new();
    file.take((MAX_COLLECTION_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| UNAVAILABLE)?;
    CollectionRequest::decode(&bytes).ok_or(UNAVAILABLE)
}
