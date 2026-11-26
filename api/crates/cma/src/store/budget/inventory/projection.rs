use super::{Result, Value, INVALID};
use crate::store::{agent_log_integrity::QUARANTINE_FIELD, agents::read_agent};
use mongodb::bson::Document;
use serde_json::json;

pub(super) fn turn(document: &Document) -> Result<super::super::TurnRecord> {
    let saved = super::super::certification::read(document).map_err(|_| INVALID)?;
    let sequence = saved
        .turn_sequence
        .filter(|value| *value > 0)
        .ok_or(INVALID)?;
    let valid_id = |id: &str| {
        !id.is_empty() && id.len() <= 256 && id.trim() == id && !id.chars().any(char::is_control)
    };
    if !valid_id(saved.owner.as_str())
        || !valid_id(saved.agent_id.as_str())
        || document.get_str("_id").ok()
            != Some(format!("turn:{}:{sequence}", saved.agent_id).as_str())
        || saved.reservation.policy.validate().is_err()
        || saved.period_shard.is_some() && saved.period_start.is_none()
    {
        return Err(INVALID);
    }
    Ok(saved)
}

pub(super) fn exact_saved(saved: &super::super::TurnRecord) -> Result<Value> {
    fn decimal(value: &mut Value) -> Result<()> {
        match value {
            Value::Number(number) => {
                *value = Value::String(number.as_u64().ok_or(INVALID)?.to_string())
            }
            Value::Array(items) => {
                for item in items {
                    decimal(item)?;
                }
            }
            Value::Object(fields) => {
                for item in fields.values_mut() {
                    decimal(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut value = serde_json::to_value(saved).map_err(|_| INVALID)?;
    decimal(&mut value)?;
    Ok(value)
}

pub(super) fn linkage(
    saved: &super::super::TurnRecord,
    document: Option<&Document>,
) -> (&'static str, Value) {
    let Some(document) = document else {
        return ("agent_missing", Value::Null);
    };
    if document.contains_key(QUARANTINE_FIELD) {
        return ("agent_quarantined", Value::Null);
    }
    let Ok(agent) = read_agent(document) else {
        return ("agent_unreadable", Value::Null);
    };
    if agent.validate_conversation().is_err() {
        return ("agent_unreadable", Value::Null);
    }
    if agent.id != saved.agent_id || agent.owner != saved.owner {
        return ("agent_identity_conflict", Value::Null);
    }
    let active = agent.conversation.active_turn;
    let generation = agent.conversation.runtime_generation;
    let outcome = if active.is_none() {
        "no_active_turn"
    } else if active != saved.turn_sequence {
        "different_active_turn"
    } else if generation.is_none() || saved.runtime_generation.is_none() {
        "generation_missing"
    } else if generation != saved.runtime_generation {
        "generation_changed"
    } else {
        "matching_active_turn"
    };
    (
        outcome,
        json!({"revision": agent.revision.to_string(), "status": agent.status,
        "active_turn": active.map(|value| value.to_string()),
        "runtime_generation": generation.map(|value| value.to_string())}),
    )
}
