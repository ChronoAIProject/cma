use crate::sandbox::AgentLogEntry;
use std::io::{self, Write};

/// Optional suggestion work must never retain an arbitrarily large source turn.
pub const MAX_CONTEXT_BYTES: usize = 256 * 1024;

/// Measure serialized source without allocating a second copy of its content.
pub fn context_entry_bytes(entry: &AgentLogEntry) -> Option<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|total| *total <= MAX_CONTEXT_BYTES)
                .ok_or_else(|| io::Error::other("suggestion context exceeds byte budget"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, entry).ok()?;
    Some(counter.0)
}
