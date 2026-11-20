use super::*;
use std::fmt::Write as _;

impl HostedContextRefresh {
    pub(super) fn receipt(&self, status: &RuntimeStatus, session: &str, sequence: u64) -> String {
        let mut hash = Sha256::new();
        // Tuple serialization has the same array encoding as the historical
        // Value tree. Borrow every field and hash bytes as the serializer emits
        // them, so the only heap allocation is the fixed hexadecimal result.
        serde_json::to_writer(
            &mut hash,
            &(
                "managed-context-v1",
                &self.0.handle.0,
                &status.sandbox_id,
                status.generation,
                &status.home_identity,
                &status.boot_id,
                &self.0.path,
                session,
                sequence,
            ),
        )
        .expect("borrowed scalar receipt fields serialize to a digest infallibly");
        let mut receipt = String::with_capacity(64);
        write!(&mut receipt, "{:x}", hash.finalize()).expect("String formatting is infallible");
        receipt
    }
}
