use std::io::{BufRead as _, BufReader, Read, Seek as _};
use std::path::Path;

use flate2::bufread::GzDecoder;

use super::{archive_io, invalid};
use crate::failure::Result;

pub(super) enum ArchiveReader {
    Plain(BufReader<std::fs::File>),
    Gzip(Box<GzDecoder<BufReader<std::fs::File>>>),
}

impl ArchiveReader {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let mut source =
            std::fs::File::open(path).map_err(archive_io("open checkpoint archive"))?;
        let mut magic = [0; 2];
        source
            .read_exact(&mut magic)
            .map_err(|_| invalid("checkpoint archive is truncated"))?;
        let compressed = magic == [0x1f, 0x8b];
        if compressed {
            let mut header = [0; 8];
            source
                .read_exact(&mut header)
                .map_err(|_| invalid("compressed checkpoint header is truncated"))?;
            // Fixed headers prevent unbounded gzip filename/comment/extra-field allocation.
            if header[0] != 8 || header[1] != 0 {
                return Err(invalid("compressed checkpoint header is invalid"));
            }
        }
        source
            .rewind()
            .map_err(archive_io("rewind checkpoint archive"))?;
        let source = BufReader::new(source);
        Ok(if compressed {
            Self::Gzip(Box::new(GzDecoder::new(source)))
        } else {
            Self::Plain(source)
        })
    }

    pub(super) fn is_compressed(&self) -> bool {
        matches!(self, Self::Gzip(_))
    }

    pub(super) fn skip_exact(&mut self, bytes: u64) -> Result<()> {
        let copied = std::io::copy(&mut self.take(bytes), &mut std::io::sink())
            .map_err(|_| invalid("checkpoint archive is truncated or corrupt"))?;
        if copied != bytes {
            return Err(invalid("checkpoint archive is truncated"));
        }
        Ok(())
    }

    pub(super) fn verify_payload(&mut self, bytes: u64) -> Result<()> {
        let copied = std::io::copy(&mut (&mut *self).take(bytes + 1), &mut std::io::sink())
            .map_err(|_| invalid("compressed checkpoint payload is truncated or corrupt"))?;
        if copied != bytes {
            return Err(invalid(
                "checkpoint payload length does not match its manifest",
            ));
        }
        self.finish()
    }

    pub(super) fn finish(&mut self) -> Result<()> {
        if self
            .read(&mut [0])
            .map_err(|_| invalid("checkpoint archive is truncated or corrupt"))?
            != 0
        {
            return Err(invalid("checkpoint archive contains trailing payload"));
        }
        if let Self::Gzip(decoder) = self {
            if !decoder
                .get_mut()
                .fill_buf()
                .map_err(|_| invalid("checkpoint archive is truncated or corrupt"))?
                .is_empty()
            {
                return Err(invalid(
                    "compressed checkpoint contains trailing bytes or members",
                ));
            }
        }
        Ok(())
    }
}

impl Read for ArchiveReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(source) => source.read(buffer),
            Self::Gzip(source) => source.read(buffer),
        }
    }
}
