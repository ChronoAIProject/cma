use serde::Deserialize;

use super::records::Records;

pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn decimal(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|n| n != 0 && n.to_string() == value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Seal {
    format: String,
    restore_admission: String,
    semantic_reconciliation: String,
    pub(super) payload_sha256: String,
    pub(super) payload_bytes: u64,
    pub(super) inventory_sha256: String,
    pub(super) source_records: usize,
}

impl Seal {
    pub(super) fn valid(&self) -> bool {
        self.format == "runtime-ledger-v1-evidence-seal-v1"
            && self.restore_admission == "HELD"
            && self.semantic_reconciliation == "NOT_RUN"
            && digest(&self.payload_sha256)
            && digest(&self.inventory_sha256)
            && self.payload_bytes <= super::super::OUTPUT_BYTES as u64
            && self.source_records > 0
            && self.source_records <= super::super::super::Limits::default().entries
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Payload {
    format: String,
    scope: String,
    enumeration_complete: bool,
    restore_admission: String,
    semantic_reconciliation: String,
    native_command_correlation: String,
    native_session_inventory: String,
    runtime_output_boundary: String,
    retirement_inventory: String,
    full_home_inventory: String,
    provider_identity: String,
    writer_exclusion: String,
    external_immutability: String,
    mutation_histories: String,
    auxiliary_recovery: String,
    sandbox_id: String,
    #[serde(deserialize_with = "required_home")]
    home_identity: Option<String>,
    generation: String,
    pub(super) inventory_sha256: String,
    pub(super) source_records: usize,
    source_directories: usize,
    records: Records,
}

fn required_home<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

impl Payload {
    pub(super) fn valid(&self) -> bool {
        self.format == "runtime-ledger-v1-evidence-v1"
            && self.scope == "v1_mutation_and_auxiliary_metadata_only"
            && self.enumeration_complete
            && self.restore_admission == "HELD"
            && self.semantic_reconciliation == "NOT_RUN"
            && self.native_command_correlation == "UNKNOWN"
            && self.native_session_inventory == "NOT_ASSESSED"
            && self.runtime_output_boundary == "UNKNOWN"
            && self.retirement_inventory == "UNSUPPORTED"
            && self.full_home_inventory == "NOT_ASSESSED"
            && self.provider_identity == "UNVERIFIED"
            && self.writer_exclusion == "UNVERIFIED"
            && self.external_immutability == "UNVERIFIED"
            && self.mutation_histories == "VALIDATED"
            && self.auxiliary_recovery == "UNQUALIFIED"
            && !self.sandbox_id.trim().is_empty()
            && self.sandbox_id.len() <= 256
            && self
                .home_identity
                .as_ref()
                .is_none_or(|v| layer3_adapter::valid_home_identity(v))
            && decimal(&self.generation)
            && digest(&self.inventory_sha256)
            && self.source_records == self.records.0
            && self.source_records > 0
            && self.source_directories > 0
            && self.source_directories <= super::super::super::Limits::default().entries
    }
}
