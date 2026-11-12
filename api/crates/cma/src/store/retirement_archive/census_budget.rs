//! Sandbox budget variants and independent account/platform accounting.
use super::*;

// rfc3339_at emits UTC whole seconds, without offsets or fractional seconds.
// Match calendar dates (including Gregorian leap years) as well as clock bounds.
const RFC3339_UTC_SECONDS: &str = concat!(
    r"(?:[0-9]{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12][0-9]|3[01])|",
    r"(?:0[469]|11)-(?:0[1-9]|[12][0-9]|30)|02-(?:0[1-9]|1[0-9]|2[0-8]))|",
    r"(?:[0-9]{2}(?:0[48]|[2468][048]|[13579][26])|(?:[02468][048]|[13579][26])00)-02-29)",
    r"T(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z"
);

pub(super) async fn require_known_shapes(db: &Database, session: &mut ClientSession) -> Result<()> {
    let known = doc! {"$or":[
        {"_id":{"$regex":"^owner:.+$"}},
        {"_id":{"$regex":"^command:.+$"},"record.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^turn:[^:]+:[0-9]+$"},"record.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^request:[^:]+:.+$"},"record.hold.scope.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^request-turn:[^:]+:[0-9]+$"}},
        // Query $type also matches array elements; stop evidence must be a scalar string.
        {"_id":{"$regex":r"\Astop:[^:]+:[0-9]+\z"},
         "$expr":{"$eq":[{"$type":"$record"},"string"]}},
        // #1741's period and observation writers use rfc3339_at. These keys
        // contain colons inside timestamps and carry no Sandbox ownership.
        {"_id":{"$regex":format!(r"\Aplatform:{RFC3339_UTC_SECONDS}\z")}},
        {"_id":{"$regex":format!(r"\Aobservation:cloud:{RFC3339_UTC_SECONDS}:{RFC3339_UTC_SECONDS}\z")}}
    ]};
    if db
        .collection::<Document>("model_budget_reservations")
        .find_one(doc! {"$nor":[known]})
        .projection(doc! {"_id":1})
        .max_time(std::time::Duration::from_secs(30))
        .session(session)
        .await
        .map_err(|_| REFUSED)?
        .is_some()
    {
        return Err(REFUSED);
    }
    Ok(())
}
