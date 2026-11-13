use super::*;
use std::path::Path;

pub(super) fn load(path: &Path, agent: &str) -> Result<Record> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Record {
                version: 1,
                ..Record::default()
            })
        }
        Err(_) => return Err(corrupt()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4_096 {
        return Err(corrupt());
    }
    let record: Record = serde_json::from_slice(&std::fs::read(path).map_err(|_| corrupt())?)
        .map_err(|_| corrupt())?;
    if record.version != 1
        || record
            .owner
            .as_ref()
            .is_none_or(|owner| owner.agent_id != agent || owner.validate().is_err())
    {
        return Err(corrupt());
    }
    Ok(record)
}

pub(super) fn save(path: &Path, record: &Record) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().ok_or_else(corrupt)?;
    crate::fs_durability::create_dir_all(parent).map_err(|_| corrupt())?;
    let metadata = std::fs::symlink_metadata(parent).map_err(|_| corrupt())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(corrupt());
    }
    let temporary = parent.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let bytes = serde_json::to_vec(record).map_err(|_| corrupt())?;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|_| corrupt())?;
        file.write_all(&bytes).map_err(|_| corrupt())?;
        file.sync_all().map_err(|_| corrupt())?;
        crate::fs_durability::rename(&temporary, path).map_err(|_| corrupt())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn corrupt() -> RuntimeError {
    RuntimeError::unavailable(
        "manager_ownership_record_invalid",
        "Agent Manager authority record could not be read or persisted safely",
    )
}
