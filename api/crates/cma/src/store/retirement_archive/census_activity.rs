//! Resource targets use raw IDs, sandbox:<ID>, and concrete HTTP path segments.
//! MatchedPath templates and account/platform audits contain no source identity.
use super::*;

pub(super) async fn selection(
    db: &Database,
    closure: &Closure,
    session: &mut ClientSession,
) -> Result<Document> {
    // URI encoding can conceal a source ID from a literal segment comparison.
    // No canonicalization of ambiguous retained inputs is invented during export.
    if db
        .collection::<Document>("activity")
        .find_one(doc! {
            "record.target.kind":"resource", "record.target.id":{"$regex":"^/.*%"}
        })
        .projection(doc! {"_id":1})
        .max_time(std::time::Duration::from_secs(30))
        .session(session)
        .await
        .map_err(|_| REFUSED)?
        .is_some()
    {
        return Err(REFUSED);
    }
    let mut targets = closure.resources.clone();
    targets.extend(closure.sandbox.iter().map(|id| format!("sandbox:{id}")));
    Ok(doc! {"record.target.kind":"resource", "$or":[
        {"record.target.id":{"$in":targets}},
        {"record.target.id":{"$regex":"^/api/v1/"},"$expr":{"$gt":[
            {"$size":{"$setIntersection":[
                {"$split":[{"$ifNull":["$record.target.id",""]},"/"]},
                &closure.resources
            ]}},0
        ]}}
    ]})
}
