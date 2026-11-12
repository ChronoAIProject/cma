//! Reservation variants and independent platform counter accounting.
use super::*;

/// Canonical nonnegative BSON signed seconds and one of the sixteen fixed
/// partitions. Conversion refuses numeric overflow; the anchored grammar
/// rejects signs, leading zeroes, suffixes and future partition layouts.
fn counter_shape() -> Document {
    doc! {
        "_id": {"$regex": r"\Aplatform-counter:(?:0|[1-9][0-9]{0,18}):(?:[0-9]|1[0-5])\z"},
        "$expr": {"$ne": [{"$convert": {
            "input": {"$arrayElemAt": [{"$split": [
                {"$convert": {"input": "$_id", "to": "string", "onError": "", "onNull": ""}}, ":"
            ]}, 1]},
            "to": "long", "onError": Bson::Null, "onNull": Bson::Null
        }}, Bson::Null]}
    }
}

pub(super) async fn require_known_shapes(db: &Database, session: &mut ClientSession) -> Result<()> {
    let known = doc! {"$or":[
        {"_id":{"$regex":"^owner:.+$"}},
        {"_id":{"$regex":"^command:.+$"},"record.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^turn:[^:]+:[0-9]+$"},"record.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^request:[^:]+:.+$"},"record.hold.scope.agent_id":{"$type":"string"}},
        {"_id":{"$regex":"^request-turn:[^:]+:[0-9]+$"}},
        counter_shape()
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
