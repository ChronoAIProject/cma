//! Exact native setup evidence supplements a missing durable confirmation only.
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::acp::Bridge;
use crate::AdapterError;

/// Opt-in contract demonstrated by a provider against its pinned runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedOpenCompletionProtocol {
    V1,
}

#[derive(serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CompletionReceipt {
    complete: bool,
    version: u64,
    session_id: String,
    challenge: String,
    binding_digest: String,
    process_nonce: String,
    open_generation: String,
    session_generation: u64,
}

impl Bridge {
    pub(super) async fn read_open_completion(
        &self,
        protocol: HostedOpenCompletionProtocol,
        conversation: &str,
        digest: &str,
    ) -> crate::Result<CompletionReceipt> {
        let HostedOpenCompletionProtocol::V1 = protocol;
        let challenge = uuid::Uuid::new_v4().to_string();
        // This uses the same authenticated, boot-bound socket as initialize.
        // No source is re-resolved and no native close/load/prompt is issued.
        let result = self
            .call_within(
                "cma/open-completion",
                json!({"action":"read", "sessionId":conversation,
                    "bindingDigest":digest, "challenge":challenge}),
                Duration::from_secs(3),
            )
            .await
            .map_err(|_| unavailable())?;
        let receipt: CompletionReceipt =
            CompletionReceipt::deserialize(&*result).map_err(|_| unavailable())?;
        if !receipt.complete
            || receipt.version != 1
            || receipt.session_id != conversation
            || receipt.challenge != challenge
            || receipt.binding_digest != digest
            || !uuid4(&receipt.process_nonce)
            || !uuid4(&receipt.open_generation)
            || receipt.session_generation > 9_007_199_254_740_991
        {
            return Err(unavailable());
        }
        Ok(receipt)
    }

    pub(super) async fn recheck_open_completion(
        &self,
        protocol: HostedOpenCompletionProtocol,
        previous: CompletionReceipt,
    ) -> crate::Result<()> {
        let mut current = self
            .read_open_completion(protocol, &previous.session_id, &previous.binding_digest)
            .await?;
        // Every read has its own challenge; all native incarnation fields must
        // remain identical across handshake settings and before publication.
        current.challenge = previous.challenge.clone();
        if current != previous {
            return Err(unavailable());
        }
        Ok(())
    }
}

pub(super) fn binding_digest(cwd: &str, servers: &[Value]) -> crate::Result<String> {
    if !std::path::Path::new(cwd).is_absolute() || servers.len() > 32 {
        return Err(unavailable());
    }
    let canonical = canonical_json(&json!({"cwd":cwd, "mcpServers":servers}))?;
    if canonical.len() > 65_536 {
        return Err(unavailable());
    }
    Ok(format!("v1:{:x}", Sha256::digest(canonical.as_bytes())))
}

fn canonical_json(value: &Value) -> crate::Result<String> {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            let fields = keys
                .into_iter()
                .map(|key| Ok(format!("{}:{}", json!(key), canonical_json(&object[key])?)))
                .collect::<crate::Result<Vec<_>>>()?;
            Ok(format!("{{{}}}", fields.join(",")))
        }
        Value::Array(values) => Ok(format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<crate::Result<Vec<_>>>()?
                .join(",")
        )),
        // Current typed HTTP descriptors have no numbers. Never assume that
        // Rust numeric serialization equals ECMAScript numeric serialization.
        Value::Number(_) => Err(unavailable()),
        _ => Ok(value.to_string()),
    }
}

fn uuid4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version_num() == 4
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == value
    })
}

fn unavailable() -> AdapterError {
    AdapterError::Unavailable("exact completed conversation setup is not available".into())
}

#[cfg(test)]
#[path = "acp_open_completion_digest_tests.rs"]
mod digest_tests;
