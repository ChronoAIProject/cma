use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub partition: String,
    pub head_work: String,
    pub head_version: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub binding: Binding,
    pub request_id: String,
    pub reason: String,
    #[serde(default)]
    pub plan_digest: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub custody: String,
    pub binding: Binding,
    pub partition_hash: String,
    pub target: String,
    pub state: String,
    pub age_seconds: i64,
    pub queued_effect_count: usize,
    pub outstanding_quarantined_effects: usize,
    pub rollback_safe_after: String,
    pub lease_live: bool,
    pub guard_live: bool,
    pub eligible: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub execution_version: u64,
    pub guard_version: u64,
    pub effect_version: Option<u64>,
    pub execution: Value,
    pub guard: Value,
    pub effect: Option<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub kind: String,
    pub authority_key_id: u8,
    pub request: Request,
    pub target: String,
    pub disposition: String,
    pub timestamp: String,
    pub rollback_safe_after: String,
    pub prior_state: String,
    pub evidence: Vec<String>,
    pub evidence_sha256: String,
    pub owner: String,
    pub retained_until: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub kind: String,
    pub deployment: String,
    pub audits: Vec<String>,
    pub deadlines: std::collections::BTreeMap<String, i64>,
    pub rollback_safe_after: Option<String>,
}
#[derive(Serialize)]
pub struct Inspection {
    pub outstanding_lane_items: usize,
    pub lanes: Vec<LaneView>,
    pub skipped: usize,
    pub truncated: bool,
    pub candidates: Vec<Candidate>,
    pub unreadable: Vec<Unreadable>,
    pub abandonments: Vec<Abandonment>,
    pub rollback_safe_after: Option<String>,
    pub outstanding_quarantined_effects: usize,
    pub members: Vec<crate::replicas::membership::MemberCapability>,
}
#[derive(Serialize)]
pub struct Abandonment {
    pub audit: String,
    pub target: String,
    pub rollback_safe_after: String,
    pub outstanding_effects: usize,
}
#[derive(Serialize)]
pub struct Preview {
    pub execution_version: u64,
    pub guard_id: String,
    pub guard_version: u64,
    pub item_version: u64,
    pub queued_effect_ids: Vec<String>,
    pub evidence_sha256: String,
    pub candidate: Candidate,
    pub plan_digest: String,
    pub changes: [&'static str; 5],
    pub members: Vec<crate::replicas::membership::MemberCapability>,
}
pub fn utc(seconds: i64) -> Result<String, super::Error> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| super::Error::Corrupt)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| super::Error::Corrupt)
}

/// Read-only observations, never bindings that authorize operator release.
#[derive(Serialize)]
pub struct Unreadable {
    pub partition: String,
    pub partition_hash: String,
    pub head_version: Option<u64>,
    pub head_work: Option<String>,
    pub class: crate::replicas::diagnostics::Class,
    pub stage: crate::replicas::diagnostics::Stage,
    pub age_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custody: Option<String>,
}

#[derive(Serialize)]
pub struct LaneView {
    pub id: String,
    pub target: String,
    pub depth: usize,
    pub oldest_age_seconds: i64,
    pub lease_until_ms: Option<i64>,
    pub retained_until: String,
}
