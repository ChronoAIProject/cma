//! Durable requests and receipts. Pending payloads are immutable across retries.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FundingError {
    #[error("Sandbox is outside paid admission")]
    NotEnrolled,
    #[error("prepaid funding temporarily unavailable: {0}")]
    Retryable(String),
    #[error("prepaid window not funded: {0}")]
    Unfunded(String),
    #[error("invalid prepaid funding contract: {0}")]
    Invalid(String),
    #[error("funding state changed; retry with current authority")]
    Conflict,
    #[error("prepaid producer rate limited; retry in {0} seconds")]
    RateLimited(u64),
    #[error("funding receipt not found")]
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FundingQuote {
    pub quote_id: String,
    pub owner_id: String,
    pub sandbox_id: String,
    pub maximum_allocation: Allocation,
    pub accept_before: u64,
    /// Retain the entire provider quote, including versioned price evidence.
    pub receipt: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FundingWindow {
    pub request_id: String,
    pub quote_id: String,
    pub window_id: String,
    pub owner_id: String,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub allocation: Allocation,
    pub covered_from: u64,
    pub covered_until: u64,
    pub receipt: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuoteRequest {
    pub owner_id: String,
    pub sandbox_id: String,
    pub allocation: Allocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReserveRequest {
    pub request_id: String,
    pub quote_id: String,
    pub owner_id: String,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub operation_id: String,
    pub previous_window_id: Option<String>,
    pub seconds: u64,
    pub allocation: Allocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceInterval {
    pub revision: u64,
    pub started_at: u64,
    pub ended_at: u64,
    pub allocation: Allocation,
    pub quantity: layer2_core::sandbox_usage::UsageTotals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceRequest {
    pub window_id: String,
    pub owner_id: String,
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub intervals: Vec<EvidenceInterval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RetainedWindow {
    pub window: FundingWindow,
    pub evidence: Option<EvidenceRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SandboxBillingState {
    pub sandbox_id: String,
    pub owner_id: String,
    /// Compare-and-swap version; every persisted change increments it.
    pub revision: u64,
    pub source_operation: String,
    pub operation_id: String,
    pub runtime_generation: u64,
    /// A confirmed exclusion pins this generation to the ordinary unbilled lifecycle.
    #[serde(default)]
    pub unbilled_generation: Option<u64>,
    pub quote: Option<FundingQuote>,
    pub pending: Option<ReserveRequest>,
    pub current: Option<FundingWindow>,
    pub coverage_start: u64,
    /// Ordered receipts, removed only after the exact frozen evidence is accepted.
    pub windows: Vec<RetainedWindow>,
    pub evidence_cursor: u64,
    /// Initial prospective enrollment may scan older retained history in bounded pages.
    #[serde(default)]
    pub evidence_scan_cursor: u64,
    #[serde(default)]
    pub retained_disk_gib: u32,
    pub stopped: bool,
    pub last_error: Option<String>,
    pub retry_at: u64,
    pub attempts: u32,
}

impl SandboxBillingState {
    pub fn new(sandbox: &Sandbox, generation: u64) -> Self {
        Self {
            sandbox_id: sandbox.id.to_string(),
            owner_id: sandbox.owner.to_string(),
            revision: 0,
            source_operation: source_operation(sandbox),
            operation_id: uuid::Uuid::new_v4().to_string(),
            runtime_generation: generation,
            unbilled_generation: None,
            quote: None,
            pending: None,
            current: None,
            coverage_start: 0,
            windows: Vec::new(),
            evidence_cursor: 0,
            evidence_scan_cursor: 0,
            retained_disk_gib: maximum_allocation(sandbox).disk_gib,
            stopped: false,
            last_error: None,
            retry_at: 0,
            attempts: 0,
        }
    }
}

pub(super) fn seconds(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub(super) fn source_operation(sandbox: &Sandbox) -> String {
    sandbox
        .lifecycle
        .operation
        .as_ref()
        .map(|op| op.authority().operation_id.clone())
        .unwrap_or_else(|| format!("{}:initial", sandbox.id))
}

pub(super) fn generation(sandbox: &Sandbox) -> u64 {
    sandbox
        .lifecycle
        .runtime_provision
        .as_ref()
        .map(|r| r.generation)
        .or_else(|| {
            sandbox
                .runtime
                .as_ref()
                .and_then(|r| r.provision_generation)
        })
        .unwrap_or(sandbox.lifecycle.runtime_generation_high_water.max(1))
}

pub(super) fn maximum_allocation(sandbox: &Sandbox) -> Allocation {
    Allocation {
        running: true,
        cpu_millis: sandbox.profile.content.cpu_millis,
        memory_mib: sandbox.profile.content.memory_mib,
        disk_gib: sandbox
            .lifecycle
            .home
            .as_ref()
            .map_or(0, |h| h.request.policy.capacity_gib),
    }
}

pub(super) fn validate_receipt(
    request: &ReserveRequest,
    receipt: &FundingWindow,
    previous: Option<&FundingWindow>,
) -> Result<(), FundingError> {
    let duration = receipt.covered_until.checked_sub(receipt.covered_from);
    if receipt.request_id != request.request_id
        || receipt.quote_id != request.quote_id
        || receipt.owner_id != request.owner_id
        || receipt.sandbox_id != request.sandbox_id
        || receipt.runtime_generation != request.runtime_generation
        || receipt.allocation != request.allocation
        || receipt.window_id.is_empty()
        || duration.is_none_or(|d| d == 0 || d > request.seconds)
        || previous.is_some_and(|p| {
            receipt.covered_from < p.covered_until
                || (p.runtime_generation == request.runtime_generation
                    && receipt.covered_from != p.covered_until)
        })
    {
        return Err(FundingError::Invalid(
            "receipt differs from persisted request or coverage".into(),
        ));
    }
    Ok(())
}
