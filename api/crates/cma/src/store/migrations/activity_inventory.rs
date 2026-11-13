//! Bounded, strict inventory for the Activity page index operation.

use mongodb::{
    bson::{doc, Document},
    Database,
};

use super::{activity_page_index, Result};

pub(super) async fn strict_pending(database: &Database) -> Result<Vec<Document>> {
    let response = database
        .run_command(doc! {
            "listIndexes": "activity",
            "cursor": {"batchSize": 64},
            "maxTimeMS": 5000_i32,
        })
        .await
        .map_err(|_| "Activity index inventory unavailable")?;
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "Activity index cursor malformed")?;
    if cursor
        .get_i64("id")
        .or_else(|_| cursor.get_i32("id").map(i64::from))
        .map_err(|_| "Activity index cursor id malformed")?
        != 0
    {
        return Err("Activity index inventory exceeds bound");
    }
    let existing = cursor
        .get_array("firstBatch")
        .map_err(|_| "Activity index batch malformed")?
        .iter()
        .map(|value| {
            value
                .as_document()
                .cloned()
                .ok_or("Activity index entry malformed")
        })
        .collect::<Result<Vec<_>>>()?;
    strict_missing(&existing)
}

pub(super) fn strict_missing(existing: &[Document]) -> Result<Vec<Document>> {
    if existing
        .iter()
        .any(|index| index.get_str("name").ok() == Some("_id_") && index.contains_key("collation"))
    {
        return Err("Activity collection has an unreviewed default collation");
    }
    let desired = activity_page_index::indexes()[0].clone();
    let mut matches = existing.iter().filter(|index| {
        index.get("name") == desired.get("name")
            || same_keys(
                index.get_document("key").ok(),
                desired.get_document("key").ok(),
            )
    });
    let Some(index) = matches.next() else {
        return Ok(vec![desired]);
    };
    if matches.next().is_some()
        || index.get("name") != desired.get("name")
        || !same_keys(
            index.get_document("key").ok(),
            desired.get_document("key").ok(),
        )
        || index
            .keys()
            .any(|key| !["v", "ns", "name", "key"].contains(&key.as_str()))
    {
        return Err("Activity index key or option drift");
    }
    Ok(Vec::new())
}

fn same_keys(left: Option<&Document>, right: Option<&Document>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.iter().eq(right.iter()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use mongodb::bson::doc;

    use super::{activity_page_index, strict_missing};

    #[test]
    fn exact_existing_index_is_a_zero_change_reconciliation() {
        let index = activity_page_index::indexes()[0].clone();
        assert_eq!(strict_missing(&[]).unwrap().len(), 1);
        assert!(strict_missing(std::slice::from_ref(&index))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn strict_inventory_refuses_option_and_collection_collation_drift() {
        let mut drift = activity_page_index::indexes()[0].clone();
        drift.insert("background", false);
        assert!(strict_missing(&[drift]).is_err());
        let mut reordered = activity_page_index::indexes()[0].clone();
        reordered.insert("key", doc! {"_id":-1,"recorded_at":-1});
        assert!(strict_missing(&[reordered]).is_err());
        assert!(strict_missing(&[doc! {
            "name":"_id_",
            "key":{"_id":1},
            "collation":{"locale":"en"}
        }])
        .is_err());
    }
}
