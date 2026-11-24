use std::io::{BufReader, Read};

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use super::{invalid, Budget, Directory, Identity, Result};

pub(super) fn read<T: DeserializeOwned>(
    directory: &Directory,
    name: &str,
    maximum: u64,
    budget: &mut Budget,
) -> Result<(T, String, Identity)> {
    let before = directory.file_identity(name)?;
    if before.bytes > maximum || !before.sealed_file() {
        return Err(invalid());
    }
    let file = directory.open_at(name, 0)?.ok_or_else(invalid)?;
    if Identity::from(file.metadata().map_err(|_| invalid())?) != before {
        return Err(invalid());
    }
    budget.file(before.bytes)?;
    struct Reader<'a, R> {
        source: R,
        budget: &'a Budget,
        hash: Sha256,
        bytes: u64,
    }
    impl<R: Read> Read for Reader<'_, R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.budget
                .check()
                .map_err(|_| std::io::ErrorKind::TimedOut)?;
            let length = self.source.read(buffer)?;
            self.hash.update(&buffer[..length]);
            self.bytes += length as u64;
            Ok(length)
        }
    }
    let mut reader = Reader {
        source: BufReader::new((&file).take(maximum + 1)),
        budget,
        hash: Sha256::new(),
        bytes: 0,
    };
    let result = serde_json::from_reader(&mut reader);
    reader.budget.check()?;
    let value = result.map_err(|_| invalid())?;
    if reader.bytes != before.bytes
        || Identity::from(file.metadata().map_err(|_| invalid())?) != before
        || directory.file_identity(name)? != before
    {
        return Err(invalid());
    }
    Ok((value, crate::command::hex(&reader.hash.finalize()), before))
}
