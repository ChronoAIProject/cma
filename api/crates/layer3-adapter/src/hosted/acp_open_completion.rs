//! Exact native setup evidence supplements a missing durable confirmation only.
use crate::ingress_budget::diagnostic::text;
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

pub(super) enum RetainedCompletion {
    Legacy(CompletionReceipt),
    Admitted(crate::ingress_budget::SharedOwned<CompletionReceipt>),
}

impl std::ops::Deref for RetainedCompletion {
    type Target = CompletionReceipt;
    fn deref(&self) -> &CompletionReceipt {
        match self {
            Self::Legacy(receipt) => receipt,
            Self::Admitted(receipt) => receipt,
        }
    }
}

impl Bridge {
    pub(super) async fn read_open_completion(
        &self,
        protocol: HostedOpenCompletionProtocol,
        conversation: &str,
        digest: &str,
    ) -> crate::Result<RetainedCompletion> {
        let HostedOpenCompletionProtocol::V1 = protocol;
        let _request_storage = super::acp::ingress::identity::json_storage(
            self.ingress.as_ref(),
            &(conversation, digest, uuid::Uuid::nil()),
        )?;
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
        let receipt = match &self.ingress {
            Some(policy) => {
                let admission = super::acp::ingress::typed::TypedAdmission::new::<CompletionReceipt>(
                    &result, policy,
                )?;
                RetainedCompletion::Admitted(
                    admission
                        .decode::<CompletionReceipt>(&result)
                        .map_err(|_| unavailable())?
                        .into_shared()?,
                )
            }
            None => RetainedCompletion::Legacy(
                CompletionReceipt::deserialize(&*result).map_err(|_| unavailable())?,
            ),
        };
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
        previous: RetainedCompletion,
    ) -> crate::Result<()> {
        let current = self
            .read_open_completion(protocol, &previous.session_id, &previous.binding_digest)
            .await?;
        // Every read has its own challenge; all native incarnation fields must
        // remain identical across handshake settings and before publication.
        if current.complete != previous.complete
            || current.version != previous.version
            || current.session_id != previous.session_id
            || current.binding_digest != previous.binding_digest
            || current.process_nonce != previous.process_nonce
            || current.open_generation != previous.open_generation
            || current.session_generation != previous.session_generation
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

/// What an initialized bridge proves about its own completed opens when the
/// caller holds no conversation identifier.
pub(super) enum CurrentOpen {
    /// Exactly one completed, live open on this bridge process, with this
    /// Agent's exact binding. It is the open an interrupted attempt created.
    Adopt(String),
    /// An open is still running on this bridge; retry instead of duplicating it.
    InFlight,
    /// Nothing provable: an older runtime, no open, several, or another binding.
    Unproven,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CurrentOpens {
    version: u64,
    challenge: String,
    process_nonce: String,
    opening: bool,
    sessions: Vec<CurrentSession>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CurrentSession {
    session_id: String,
    binding_digest: String,
}

impl Bridge {
    /// Read-only: lists the process's completed opens. Adoption is verified
    /// again through the exact completion receipt before any use.
    pub(super) async fn current_open_completion(
        &self,
        protocol: HostedOpenCompletionProtocol,
        digest: &str,
    ) -> crate::Result<CurrentOpen> {
        let HostedOpenCompletionProtocol::V1 = protocol;
        let _request_storage = super::acp::ingress::identity::json_storage(
            self.ingress.as_ref(),
            &(digest, uuid::Uuid::nil()),
        )?;
        // A runtime without the action keeps today's refusal; never guess.
        let Ok(support) = self
            .call_within(
                "cma/open-completion",
                json!({"action": "support"}),
                Duration::from_secs(3),
            )
            .await
        else {
            return Ok(CurrentOpen::Unproven);
        };
        if support.get("current") != Some(&Value::Bool(true)) {
            return Ok(CurrentOpen::Unproven);
        }
        let challenge = uuid::Uuid::new_v4().to_string();
        let result = self
            .call_within(
                "cma/open-completion",
                json!({"action": "current", "challenge": challenge}),
                Duration::from_secs(3),
            )
            .await
            .map_err(|_| unavailable())?;
        let opens = match &self.ingress {
            Some(policy) => {
                let admission = super::acp::ingress::typed::TypedAdmission::new::<CurrentOpens>(
                    &result, policy,
                )?;
                let decoded = admission
                    .decode::<CurrentOpens>(&result)
                    .map_err(|_| unavailable())?;
                current_choice(&decoded, &challenge, digest)?
            }
            None => current_choice(
                &CurrentOpens::deserialize(&*result).map_err(|_| unavailable())?,
                &challenge,
                digest,
            )?,
        };
        Ok(opens)
    }
}

fn current_choice(
    opens: &CurrentOpens,
    challenge: &str,
    digest: &str,
) -> crate::Result<CurrentOpen> {
    if opens.version != 1
        || opens.challenge != challenge
        || !uuid4(&opens.process_nonce)
        || opens.sessions.len() > 32
        || opens.sessions.iter().any(|session| {
            session.session_id.is_empty()
                || session.session_id.len() > 256
                || session.session_id.chars().any(char::is_control)
                || session.binding_digest.len() != 67
        })
    {
        return Err(unavailable());
    }
    if opens.opening {
        return Ok(CurrentOpen::InFlight);
    }
    Ok(match opens.sessions.as_slice() {
        [only] if only.binding_digest == digest => CurrentOpen::Adopt(only.session_id.clone()),
        _ => CurrentOpen::Unproven,
    })
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

pub(super) enum BindingDigest {
    Legacy(String),
    Admitted(crate::ingress_budget::SharedOwned<String>),
}

impl std::ops::Deref for BindingDigest {
    type Target = str;
    fn deref(&self) -> &str {
        match self {
            Self::Legacy(value) => value.as_str(),
            Self::Admitted(value) => value.as_str(),
        }
    }
}

pub(super) fn binding_digest_with_policy(
    cwd: &str,
    servers: &[Value],
    policy: Option<&crate::ingress_budget::IngressOwnershipPolicy>,
) -> crate::Result<BindingDigest> {
    use crate::ingress_budget::{sizing, AllocationPlan, IngressClass};
    let Some(policy) = policy else {
        return binding_digest(cwd, servers).map(BindingDigest::Legacy);
    };
    if !std::path::Path::new(cwd).is_absolute() || servers.len() > 32 {
        return Err(unavailable());
    }
    let keys = servers.iter().try_fold(0usize, |total, value| {
        total
            .checked_add(sort_keys(value, 0)?)
            .ok_or_else(unavailable)
    })?;
    let result = policy.reserve(
        IngressClass::Data,
        AllocationPlan::new(
            "open-completion digest",
            &[67, sizing::shared_owner_bytes::<String>()],
        )?,
    )?;
    // Each object allocates one exact-capacity pointer vector. Parent vectors
    // remain live during recursive values; summing the entire tree covers that
    // overlap. sort_unstable_by is in-place and does not allocate sort scratch.
    let _working = policy.reserve(
        IngressClass::Transformation,
        AllocationPlan::new(
            "canonical digest key ordering",
            &[sizing::array_bytes::<&String>(keys)?],
        )?,
    )?;
    result
        .try_initialize_static(|| streamed_digest(cwd, servers))
        .map(BindingDigest::Admitted)
        .map_err(|_| unavailable())
}

fn sort_keys(value: &Value, depth: usize) -> crate::Result<usize> {
    if depth > 64 || value.is_number() {
        return Err(unavailable());
    }
    let mut count = 0usize;
    match value {
        Value::Object(values) => {
            count = values.len();
            for value in values.values() {
                count = count
                    .checked_add(sort_keys(value, depth + 1)?)
                    .ok_or_else(unavailable)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                count = count
                    .checked_add(sort_keys(value, depth + 1)?)
                    .ok_or_else(unavailable)?;
            }
        }
        _ => {}
    }
    Ok(count)
}

struct DigestWriter {
    hash: Sha256,
    bytes: usize,
}

impl std::io::Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= 65_536)
            .ok_or(std::io::ErrorKind::InvalidData)?;
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn streamed_digest(
    cwd: &str,
    servers: &[Value],
) -> Result<String, crate::ingress_budget::AdmissionRefusal> {
    use std::{fmt::Write as _, io::Write as _};
    let mut writer = DigestWriter {
        hash: Sha256::new(),
        bytes: 0,
    };
    let encoded = (|| -> std::io::Result<()> {
        writer.write_all(b"{\"cwd\":")?;
        serde_json::to_writer(&mut writer, cwd)?;
        writer.write_all(b",\"mcpServers\":[")?;
        for (index, server) in servers.iter().enumerate() {
            if index != 0 {
                writer.write_all(b",")?;
            }
            write_canonical(&mut writer, server)?;
        }
        writer.write_all(b"]}")
    })();
    encoded.map_err(|_| crate::ingress_budget::AdmissionRefusal::InvalidPlan {
        phase: "open-completion digest",
    })?;
    let mut digest = String::with_capacity(67);
    write!(&mut digest, "v1:{:x}", writer.hash.finalize())
        .expect("String formatting is infallible");
    Ok(digest)
}

fn write_canonical(writer: &mut DigestWriter, value: &Value) -> std::io::Result<()> {
    use std::io::Write as _;
    match value {
        Value::Object(object) => {
            let mut keys = Vec::with_capacity(object.len());
            keys.extend(object.keys());
            keys.sort_unstable_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            writer.write_all(b"{")?;
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, key)?;
                writer.write_all(b":")?;
                write_canonical(writer, &object[key])?;
            }
            writer.write_all(b"}")
        }
        Value::Array(values) => {
            writer.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                write_canonical(writer, value)?;
            }
            writer.write_all(b"]")
        }
        Value::Number(_) => Err(std::io::ErrorKind::InvalidData.into()),
        _ => serde_json::to_writer(writer, value).map_err(std::io::Error::other),
    }
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
        let mut canonical = [0; 36];
        id.get_version_num() == 4
            && id.get_variant() == uuid::Variant::RFC4122
            && id.hyphenated().encode_lower(&mut canonical) == value
    })
}

fn unavailable() -> AdapterError {
    AdapterError::Unavailable(text("exact completed conversation setup is not available"))
}

#[cfg(test)]
#[path = "acp_open_completion_digest_tests.rs"]
mod digest_tests;
