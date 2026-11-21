use super::*;
pub(in crate::store) const FLOOR: &str = "title_effect_floor";
pub(in crate::store) const CUSTODY: &str = "title_effect_custody";
const MAX_BYTES: usize = 4096;

pub(in crate::store) fn initial_fields() -> Document {
    doc! {FLOOR: 1_i32, CUSTODY: Bson::Null}
}

pub(in crate::store) fn read(
    row: &Document,
) -> Result<AgentTitleEffectState, AgentTitleStoreError> {
    match (row.get(FLOOR), row.get(CUSTODY)) {
        (None, None) => Ok(AgentTitleEffectState::Legacy),
        (Some(Bson::Int32(1)), Some(Bson::Null)) => Ok(AgentTitleEffectState::Ready),
        (Some(Bson::Int32(2)), Some(Bson::Document(value))) => {
            if value.len() == 2
                && value.get_i32("version").ok() == Some(1)
                && value.get_bool("legacy_unresolved").ok() == Some(true)
            {
                return Ok(AgentTitleEffectState::LegacyUnknown);
            }

            if mongodb::bson::serialize_to_vec(value)
                .map_err(|_| AgentTitleStoreError::Corrupt)?
                .len()
                > MAX_BYTES
            {
                return Err(AgentTitleStoreError::Corrupt);
            }
            let effect: AgentTitleEffect = mongodb::bson::deserialize_from_document(value.clone())
                .map_err(|_| AgentTitleStoreError::Corrupt)?;
            effect.intent.validate()?;
            if effect.version != 1
                || row.get_str("_id").ok() != Some(effect.intent.agent_id.as_str())
            {
                return Err(AgentTitleStoreError::Corrupt);
            }
            Ok(AgentTitleEffectState::Retained(Box::new(effect)))
        }
        // Presence of either field is an irrevocable reader floor. Missing,
        // unknown-version and malformed counterparts never mean legacy retry.
        _ => Err(AgentTitleStoreError::Corrupt),
    }
}
pub(super) fn encode(effect: &AgentTitleEffect) -> Result<Document, AgentTitleStoreError> {
    effect.intent.validate()?;
    let value =
        mongodb::bson::serialize_to_document(effect).map_err(|_| AgentTitleStoreError::Corrupt)?;
    if mongodb::bson::serialize_to_vec(&value)
        .map_err(|_| AgentTitleStoreError::Corrupt)?
        .len()
        > MAX_BYTES
    {
        return Err(AgentTitleStoreError::Corrupt);
    }
    Ok(value)
}

pub(in crate::store) fn legacy_unknown_fields() -> Document {
    doc! {FLOOR: 2_i32, CUSTODY: {"version": 1_i32, "legacy_unresolved": true}}
}
