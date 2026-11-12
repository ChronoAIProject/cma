//! Persisted budget variants from budget/mod.rs, budget/requests.rs and budget/source.rs.
use super::*;

pub(super) async fn require_known_shapes(db: &Database, session: &mut ClientSession) -> Result<()> {
    let known = doc! {"$or":[
        {"_id":{"$regex":"^owner:.+$"}},
        {"_id":{"$regex":"^command:.+$"},"record.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^turn:[^:]+:[0-9]+$"},"record.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^request:[^:]+:.+$"},"record.hold.scope.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^request-turn:[^:]+:[0-9]+$"}},
        // Query $type also matches array elements; stop evidence must be a scalar string.
        {"_id":{"$regex":r"\Astop:[^:]+:[0-9]+\z"},
         "$expr":{"$eq":[{"$type":"$record"},"string"]}}
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
