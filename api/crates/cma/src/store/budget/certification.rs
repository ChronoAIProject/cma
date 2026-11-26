use mongodb::bson::Document;
use serde::{Deserialize, Serialize};

use super::{domain, ApplyError, BudgetError, TurnRecord};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Certification {
    version: u32,
    receipt: Receipt,
    outcome: Outcome,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    producer: String,
    id: String,
    digest: String,
    owner: layer2_core::SubjectId,
    agent_id: layer2_core::AgentId,
    turn_sequence: u64,
    runtime_generation: u64,
    input_tokens: u64,
    output_tokens: u64,
    model_calls: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Outcome {
    PendingTerminal {},
    RefusedConflict {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        debit: Option<OriginalDebit>,
    },
    Applied {
        debit: OriginalDebit,
        correction: i64,
    },
}

// Owner, timezone, valuation and platform period/shard come from the
// same retained parent record. `at` is the booking time, after any day rollover.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OriginalDebit {
    at: u64,
    owner_day: u64,
    units: u64,
}

pub(super) fn present(record: &Document) -> bool {
    record.contains_key("certification")
}

pub(super) fn read(row: &Document) -> Result<TurnRecord, ApplyError> {
    let record: TurnRecord = crate::store::sandbox_mongo::read_record(row)
        .map_err(|_| domain(BudgetError::Unavailable))?;
    let valid = match &record.certification {
        Some(certification) => certification.valid(&record, row.get_str("_id").ok()),
        None => !row.get_document("record").is_ok_and(present),
    };
    if !valid {
        return Err(domain(BudgetError::Unavailable));
    }
    Ok(record)
}

impl Certification {
    fn valid(&self, record: &TurnRecord, key: Option<&str>) -> bool {
        let receipt = &self.receipt;
        let reservation = &record.reservation;
        let policy = &reservation.policy;
        let bounded_id = |value: &str| {
            !value.is_empty()
                && value.len() <= 256
                && value.trim() == value
                && !value.chars().any(char::is_control)
        };
        if self.version != 1
            || !bounded_id(&receipt.producer)
            || !bounded_id(&receipt.id)
            || receipt.digest.len() != 64
            || !receipt
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !bounded_id(record.owner.as_str())
            || !bounded_id(record.agent_id.as_str())
            || receipt.owner != record.owner
            || receipt.agent_id != record.agent_id
            || receipt.turn_sequence == 0
            || Some(receipt.turn_sequence) != record.turn_sequence
            || receipt.runtime_generation == 0
            || Some(receipt.runtime_generation) != record.runtime_generation
            || key != Some(format!("turn:{}:{}", record.agent_id, receipt.turn_sequence).as_str())
            || record.period_shard.is_some() && record.period_start.is_none()
            || policy.validate().is_err()
        {
            return false;
        }
        let Ok(final_units) = policy
            .value_usage(receipt.input_tokens, receipt.output_tokens)
            .and_then(|units| i64::try_from(units).map_err(|_| BudgetError::Unavailable))
        else {
            return false;
        };
        let valid_debit = |debit: &OriginalDebit| {
            policy.day(debit.at) == Ok(debit.owner_day) && debit.at >= reservation.last_usage_at
        };
        let uncertified = !reservation.final_usage_known && reservation.final_model_calls.is_none();
        let covers_final = reservation.reserved_units >= final_units as u64;
        match &self.outcome {
            Outcome::PendingTerminal {} => {
                uncertified
                    && !reservation.settled
                    && covers_final
                    && reservation.observed <= final_units as u64
            }
            Outcome::RefusedConflict { debit } => {
                uncertified
                    && covers_final
                    && reservation.observed > final_units as u64
                    && reservation.settled == debit.is_some()
                    && debit.as_ref().is_none_or(|debit| {
                        valid_debit(debit) && debit.units == reservation.settlement_debit()
                    })
            }
            Outcome::Applied { debit, correction } => {
                reservation.settled
                    && reservation.final_usage_known
                    && reservation.final_model_calls == Some(receipt.model_calls)
                    && reservation.observed == final_units as u64
                    && valid_debit(debit)
                    && i64::try_from(debit.units)
                        .is_ok_and(|units| final_units - units == *correction)
            }
        }
    }
}
