//! Create-only synthetic sources for the exact-image transfer qualification.
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::{invalid, storage_error, SourceRecord};
use crate::failure::Result;
use crate::ledger_import::retirement_move::Pair;

pub(super) struct Sources {
    pairs: [Pair; 2],
    root: PathBuf,
}

impl Sources {
    pub(super) fn create(archive: &Path, records: &[SourceRecord]) -> Result<Self> {
        let name = archive
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(invalid)?;
        let root = archive.with_file_name(format!("{name}.sources"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(|_| storage_error())?;
        for name in [
            "active-journal",
            "active-authority",
            "retained-journal",
            "retained-authority",
        ] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(name))
                .map_err(|_| storage_error())?;
        }
        let pairs = [
            Pair::open(&root.join("active-journal"), &root.join("retained-journal"))?,
            Pair::open(
                &root.join("active-authority"),
                &root.join("retained-authority"),
            )?,
        ];
        let result = Self { pairs, root };
        result.add(records)?;
        Ok(result)
    }

    pub(super) fn add(&self, records: &[SourceRecord]) -> Result<()> {
        for record in records {
            if record.scope != 0 {
                return Err(invalid());
            }
            let directory = match record.kind {
                1 => "active-journal",
                2 => "active-authority",
                _ => return Err(invalid()),
            };
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .mode(0o600)
                .open(self.root.join(directory).join(&record.name))
                .map_err(|_| storage_error())?;
            file.write_all(&record.bytes).map_err(|_| storage_error())?;
            file.sync_all().map_err(|_| storage_error())?;
        }
        Ok(())
    }

    pub(super) fn transfer(&self, record: &SourceRecord, restore: bool) -> Result<()> {
        if record.scope != 0 {
            return Err(invalid());
        }
        let index = match record.kind {
            1 => 0,
            2 => 1,
            _ => return Err(invalid()),
        };
        self.pairs[index].transfer(&record.name, &record.bytes, restore, |_| Ok(()))
    }

    pub(super) fn counts(&self) -> Result<(usize, usize)> {
        let [journal, authority] = self.counts_by_kind()?;
        Ok((journal.0 + authority.0, journal.1 + authority.1))
    }

    pub(super) fn counts_by_kind(&self) -> Result<[(usize, usize); 2]> {
        let count = |name| -> Result<usize> {
            std::fs::read_dir(self.root.join(name))
                .map_err(|_| storage_error())?
                .try_fold(0, |count, entry| {
                    entry.map_err(|_| storage_error())?;
                    Ok(count + 1)
                })
        };
        Ok([
            (count("active-journal")?, count("retained-journal")?),
            (count("active-authority")?, count("retained-authority")?),
        ])
    }
}
