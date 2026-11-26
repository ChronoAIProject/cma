//! Agent pool v1 and machine pool v2 have independent release ordering.
use super::*;

pub(super) async fn initialize(database: &Database) -> Result<(), StoreError> {
    crate::store::schema::ensure_collections(
        database,
        &[COLLECTION, crate::store::released_runtimes::COLLECTION],
    )
    .await?;
    let unknown = doc! {"$or":[
        {"document_schema_version":{"$not":{"$type":["int","long"]}}},
        {"document_schema_version":{"$type":"array"}},
        {"machine":{"$type":"array"}},
        {"$nor":[
            {"document_schema_version":1_i64,"machine":{"$exists":false}},
            {"document_schema_version":1_i64,"machine":false},
            {"document_schema_version":2_i64,"machine":true}
        ]}
    ]};
    if database
        .collection::<Document>(COLLECTION)
        .find_one(unknown)
        .projection(doc! {"_id":1})
        .await
        .map_err(|_| StoreError::Unavailable("inspect native pool schema".into()))?
        .is_some()
    {
        return Err(StoreError::Malformed(
            "native pool schema and machine channel disagree or are unsupported".into(),
        ));
    }
    Ok(())
}
