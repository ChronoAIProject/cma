//! Immutable metadata semantics for the v1 binding migration and its readers.
//! A new contract version keeps historical migration checksums readable.
use mongodb::{
    bson::{doc, Document},
    Database,
};

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug)]
pub(in crate::store) enum InspectionError {
    Unavailable,
    DefinitionMismatch,
}

impl From<InspectionError> for &'static str {
    fn from(value: InspectionError) -> Self {
        match value {
            InspectionError::Unavailable => "schema inspection unavailable",
            InspectionError::DefinitionMismatch => "schema differs from reviewed contract",
        }
    }
}

pub(in crate::store) fn entries(response: &Document) -> Result<Vec<Document>> {
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "index cursor malformed")?;
    if cursor
        .get_i64("id")
        .or_else(|_| cursor.get_i32("id").map(i64::from))
        .map_err(|_| "index cursor id malformed")?
        != 0
    {
        return Err("index inventory exceeds bound");
    }
    cursor
        .get_array("firstBatch")
        .map_err(|_| "index batch malformed")?
        .iter()
        .map(|value| value.as_document().cloned().ok_or("index entry malformed"))
        .collect()
}

pub(in crate::store) fn missing(
    existing: &[Document],
    desired: &Document,
) -> Result<Vec<Document>> {
    if existing
        .iter()
        .any(|index| index.get_str("name").ok() == Some("_id_") && index.contains_key("collation"))
    {
        return Err("collection has an unreviewed default collation");
    }
    let same_keys = |index: &Document| {
        index
            .get_document("key")
            .ok()
            .zip(desired.get_document("key").ok())
            .is_some_and(|(left, right)| left.iter().eq(right.iter()))
    };
    let mut matches = existing
        .iter()
        .filter(|index| index.get("name") == desired.get("name") || same_keys(index));
    let Some(index) = matches.next() else {
        return Ok(vec![desired.clone()]);
    };
    if matches.next().is_some()
        || index.get("name") != desired.get("name")
        || !same_keys(index)
        || desired
            .iter()
            .any(|(key, value)| key != "key" && index.get(key) != Some(value))
        || index
            .keys()
            .any(|key| !["v", "ns"].contains(&key.as_str()) && !desired.contains_key(key))
    {
        return Err("index key or option drift");
    }
    Ok(Vec::new())
}

pub(in crate::store) async fn inspect_collection(
    database: &Database,
    collection: &str,
) -> std::result::Result<Option<Vec<Document>>, InspectionError> {
    use InspectionError::{DefinitionMismatch, Unavailable};
    let response = database
        .run_command(doc! {"listCollections":1,"filter":{"name":collection},
        "cursor":{"batchSize":2},"maxTimeMS":5000})
        .await
        .map_err(|_| Unavailable)?;
    let specifications = entries(&response).map_err(|_| Unavailable)?;
    let Some(specification) = specifications.first() else {
        return Ok(None);
    };
    if specifications.len() != 1 || specification.get_str("name").ok() != Some(collection) {
        return Err(Unavailable);
    }
    if specification.get_str("type").ok() != Some("collection")
        || !specification
            .get_document("options")
            .map_err(|_| Unavailable)?
            .is_empty()
    {
        return Err(DefinitionMismatch);
    }
    let response = database
        .run_command(doc! {"listIndexes":collection,"cursor":{"batchSize":64},
        "maxTimeMS":5000})
        .await
        .map_err(|_| Unavailable)?;
    let indexes = entries(&response).map_err(|_| Unavailable)?;
    if !missing(&indexes, &doc! {"name":"_id_","key":{"_id":1}})
        .map_err(|_| DefinitionMismatch)?
        .is_empty()
    {
        return Err(DefinitionMismatch);
    }
    Ok(Some(indexes))
}

pub(in crate::store) async fn require_binding_versions(
    database: &Database,
    collections: &[&str],
) -> Result<()> {
    for &collection in collections {
        let unsupported = doc! {"$or":[
            {"document_schema_version":{"$ne":1_i64}},
            {"document_schema_version":{"$not":{"$type":["int","long"]}}},
            {"document_schema_version":{"$type":"array"}},
        ]};
        if database
            .collection::<Document>(collection)
            .find_one(unsupported)
            .projection(doc! {"_id":1})
            .max_time(std::time::Duration::from_secs(5))
            .await
            .map_err(|_| "binding version inspection unavailable")?
            .is_some()
        {
            return Err("binding document schema incompatible");
        }
    }
    Ok(())
}
