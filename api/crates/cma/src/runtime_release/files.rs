//! Bounded private process challenges, never provider or database mutation.
use super::{
    bootstrap::{self, ProcessIdentity, VerifiedBootstrap},
    settings::Settings,
};
use crate::config::Config;
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) async fn authenticate(config: &Config) -> Result<VerifiedBootstrap, String> {
    let Settings::Managed {
        trusted_public_key,
        grant_path,
        challenge_path,
        binary_image,
    } = &config.runtime_release
    else {
        return Err("inert bootstrap requires explicit managed settings".into());
    };
    let process = ProcessIdentity {
        deployment: config.deployment_id.clone(),
        instance_id: uuid::Uuid::new_v4().to_string(),
        config_sha256: config.startup_configuration_sha256.clone(),
        binary_image: binary_image.clone(),
        executable_sha256: executable_digest()?,
        agent_image: config.runtime.sandbox_image.clone(),
        machine_image: config.runtime.machine_sandbox_image.clone(),
    };
    create_private(challenge_path, &process)?;
    // A process restart always needs a new signed challenge. The challenge is
    // retained on failure; it is not silently replaced or treated as authority.
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            match read_private(grant_path, 16 * 1024) {
                Ok(bytes) => return bootstrap::verify(&bytes, trusted_public_key, &process, now()?),
                Err(ReadError::Missing) => (),
                Err(ReadError::Invalid) => return Err("bootstrap grant is not a bounded private regular file".into()),
            }
            tokio::select! {
                () = crate::shutdown::shutdown_signal() => return Err("inert bootstrap was stopped".into()),
                () = tokio::time::sleep(Duration::from_millis(200)) => (),
            }
        }
    }).await.map_err(|_| "inert bootstrap authorization deadline expired")?
}

pub(super) fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| "runtime release clock is invalid".into())
}

pub(super) fn create_private(path: &Path, value: &impl Serialize) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = serde_json::to_vec(value).map_err(|_| "cannot encode private release receipt")?;
    let staging = path.with_extension(format!("partial-{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .map_err(|_| "cannot create private release receipt")?;
    let result = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::hard_link(&staging, path))
        .map_err(|_| "private release receipt must have a new owned path".to_owned());
    let removed = std::fs::remove_file(&staging)
        .map_err(|_| "cannot finalize private release receipt".to_owned());
    result.and(removed)
}

pub(super) enum ReadError {
    Missing,
    Invalid,
}

pub(super) fn read_private(path: &Path, limit: usize) -> Result<Vec<u8>, ReadError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let before = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReadError::Missing
        } else {
            ReadError::Invalid
        }
    })?;
    if !before.is_file() || before.permissions().mode() & 0o077 != 0 || before.len() > limit as u64
    {
        return Err(ReadError::Invalid);
    }
    let file = std::fs::File::open(path).map_err(|_| ReadError::Invalid)?;
    let after = file.metadata().map_err(|_| ReadError::Invalid)?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || after.permissions().mode() & 0o077 != 0
        || !after.is_file()
    {
        return Err(ReadError::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadError::Invalid)?;
    if bytes.len() > limit {
        return Err(ReadError::Invalid);
    }
    Ok(bytes)
}

fn executable_digest() -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let path = std::env::current_exe().map_err(|_| "cannot identify current executable")?;
    let mut file = std::fs::File::open(path).map_err(|_| "cannot read current executable")?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "cannot bind current executable bytes")?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn receipt_publication_never_replaces_an_existing_attempt() {
        let directory =
            std::env::temp_dir().join(format!("release-receipt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("receipt.json");
        create_private(&path, &serde_json::json!({"attempt": 1})).unwrap();
        assert!(create_private(&path, &serde_json::json!({"attempt": 2})).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"attempt\":1}");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn credential_inputs_refuse_symlinks_public_permissions_and_oversize() {
        let directory =
            std::env::temp_dir().join(format!("release-input-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("input.json");
        create_private(&path, &"synthetic").unwrap();
        assert!(read_private(&path, 1024).is_ok());
        assert!(matches!(read_private(&path, 2), Err(ReadError::Invalid)));
        let link = directory.join("link");
        symlink(&path, &link).unwrap();
        assert!(matches!(read_private(&link, 1024), Err(ReadError::Invalid)));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(read_private(&path, 1024), Err(ReadError::Invalid)));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
