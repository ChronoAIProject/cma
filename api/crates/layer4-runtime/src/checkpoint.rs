use std::path::{Path, PathBuf};

use layer3_adapter::{CheckpointReceipt, CheckpointRequest, RestoreRequest, WriterState};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

#[derive(Clone)]
pub(crate) struct CheckpointStore {
    root: PathBuf,
    home: PathBuf,
}

impl CheckpointStore {
    pub(crate) fn new(config: &RuntimeConfig) -> Self {
        Self {
            root: config.checkpoint_root.clone(),
            home: config.home.clone(),
        }
    }

    pub(crate) async fn capture(&self, request: &CheckpointRequest) -> Result<CheckpointReceipt> {
        let home = self.home.clone();
        let root = self.root.clone();
        let exclusions = request.exclusions.clone();
        let sealed =
            blocking(move || crate::checkpoint_archive::seal(&home, &root, &exclusions)).await?;
        tracing::info!(
            epoch = request.epoch,
            object = %sealed.reference,
            bytes = sealed.path.metadata().map(|metadata| metadata.len()).unwrap_or(0),
            "home checkpoint sealed"
        );
        Ok(CheckpointReceipt {
            epoch: request.epoch,
            object_reference: sealed.reference,
            sha256: sealed.sha256,
            writers: WriterState::Resumed,
        })
    }

    pub(crate) async fn restore(&self, request: &RestoreRequest) -> Result<()> {
        let path = self.download_path(&request.object_reference)?;
        let archive = path.clone();
        let home = self.home.clone();
        let scratch = self.root.clone();
        let expected = request.sha256.clone();
        blocking(move || restore_archive(&archive, &expected, &home, &scratch)).await
    }

    pub(crate) fn download_path(&self, reference: &str) -> Result<PathBuf> {
        self.ensure_store()?;
        let path = self.object_path(reference)?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RuntimeError::rejected(
                    "checkpoint_not_found",
                    "checkpoint object was not found",
                ))
            }
            Ok(_) => {
                return Err(RuntimeError::rejected(
                    "checkpoint_object_invalid",
                    "checkpoint object is not a regular file",
                ))
            }
            Err(error) => return Err(checkpoint_io("inspect checkpoint object")(error)),
        }
        Ok(path)
    }

    pub(crate) fn upload_path(&self, reference: &str) -> Result<(PathBuf, String)> {
        let expected = digest_from_reference(reference)?;
        self.ensure_store()?;
        let temporary = self
            .root
            .join(format!(".upload-{}-{}.tmp", std::process::id(), stamp()));
        Ok((temporary, expected))
    }

    pub(crate) async fn commit_upload(
        &self,
        temporary: PathBuf,
        reference: String,
        expected: String,
    ) -> Result<()> {
        let final_path = self.object_path(&reference)?;
        let home = self.home.clone();
        blocking(move || {
            crate::checkpoint_archive::inspect(&temporary, &expected, &home)?;
            match crate::fs_durability::hard_link(&temporary, &final_path) {
                Ok(()) => {
                    crate::fs_durability::remove_file(&temporary)
                        .map_err(checkpoint_io("remove linked checkpoint upload"))?;
                    Ok(())
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = std::fs::symlink_metadata(&final_path)
                        .map_err(checkpoint_io("inspect existing checkpoint object"))?;
                    if metadata.is_file()
                        && !metadata.file_type().is_symlink()
                        && crate::checkpoint_manifest::sha256_file(&final_path)?
                            .eq_ignore_ascii_case(&expected)
                    {
                        crate::fs_durability::remove_file(&temporary)
                            .map_err(checkpoint_io("remove duplicate checkpoint upload"))?;
                        Ok(())
                    } else {
                        Err(RuntimeError::rejected(
                            "checkpoint_collision",
                            "checkpoint object is occupied by different bytes",
                        ))
                    }
                }
                Err(error) => Err(checkpoint_io("publish checkpoint upload")(error)),
            }
        })
        .await
    }

    fn object_path(&self, reference: &str) -> Result<PathBuf> {
        validate_reference(reference)?;
        Ok(self.root.join(reference))
    }

    fn ensure_store(&self) -> Result<()> {
        crate::fs_durability::create_dir_all(&self.root)
            .map_err(checkpoint_io("create checkpoint store"))?;
        let metadata = std::fs::symlink_metadata(&self.root)
            .map_err(checkpoint_io("inspect checkpoint store"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(RuntimeError::rejected(
                "checkpoint_store_invalid",
                "checkpoint store is not a real directory",
            ));
        }
        Ok(())
    }
}

fn restore_archive(archive: &Path, expected: &str, home: &Path, scratch: &Path) -> Result<()> {
    crate::fs_durability::create_dir_all(scratch)
        .map_err(checkpoint_io("create restore scratch directory"))?;
    let staged = scratch.join(format!(
        ".restore-staging-{}-{}",
        std::process::id(),
        stamp()
    ));
    let mut cleanup = Cleanup(Some(staged.clone()));
    crate::checkpoint_archive::extract(archive, expected, home, &staged)?;
    crate::checkpoint_copy::replace_home(home, &staged, scratch)?;
    cleanup.remove_now();
    Ok(())
}

fn validate_reference(reference: &str) -> Result<()> {
    let digest = digest_from_reference(reference)?;
    crate::checkpoint_manifest::require_sha256(&digest)
}

fn digest_from_reference(reference: &str) -> Result<String> {
    let digest = reference
        .strip_prefix("home-")
        .and_then(|value| value.strip_suffix(".cma"))
        .ok_or_else(invalid_reference)?;
    if reference.len() > 96
        || reference.contains('/')
        || reference.contains('\\')
        || reference.contains('\0')
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid_reference());
    }
    Ok(digest.to_ascii_lowercase())
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| {
            RuntimeError::unavailable(
                "checkpoint_task_failed",
                format!("checkpoint filesystem task failed: {error}"),
            )
        })?
}

fn invalid_reference() -> RuntimeError {
    RuntimeError::rejected(
        "checkpoint_reference_invalid",
        "checkpoint object reference is not a local sealed-home object",
    )
}

fn checkpoint_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("checkpoint_io_failed", format!("{operation}: {error}"))
}

fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

struct Cleanup(Option<PathBuf>);

impl Cleanup {
    fn remove_now(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.remove_now();
    }
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
