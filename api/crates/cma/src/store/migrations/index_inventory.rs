//! Bounded inventory for an exact reviewed, additive index.

use mongodb::{
    bson::{doc, Document},
    Database,
};

use super::Result;

pub(super) async fn strict_pending(
    database: &Database,
    collection: &str,
    desired: &Document,
) -> Result<Vec<Document>> {
    let response = database
        .run_command(doc! {
            "listIndexes": collection,
            "cursor": {"batchSize": 64},
            "maxTimeMS": 5000_i32,
        })
        .await
        .map_err(|_| "Query index inventory unavailable")?;
    crate::store::agent_log_segments::capture::preview_budget::catalog_response(&response)?;
    strict_missing(&entries(&response)?, desired)
}

pub(super) fn entries(response: &Document) -> Result<Vec<Document>> {
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "Query index cursor malformed")?;
    if cursor
        .get_i64("id")
        .or_else(|_| cursor.get_i32("id").map(i64::from))
        .map_err(|_| "Query index cursor id malformed")?
        != 0
    {
        return Err("Query index inventory exceeds bound");
    }
    let existing = cursor
        .get_array("firstBatch")
        .map_err(|_| "Query index batch malformed")?
        .iter()
        .map(|value| {
            value
                .as_document()
                .cloned()
                .ok_or("Query index entry malformed")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(existing)
}

pub(super) fn strict_missing(existing: &[Document], desired: &Document) -> Result<Vec<Document>> {
    if existing
        .iter()
        .any(|index| index.get_str("name").ok() == Some("_id_") && index.contains_key("collation"))
    {
        return Err("Query collection has an unreviewed default collation");
    }
    let mut matches = existing.iter().filter(|index| {
        index.get("name") == desired.get("name")
            || same_keys(
                index.get_document("key").ok(),
                desired.get_document("key").ok(),
            )
    });
    let Some(index) = matches.next() else {
        return Ok(vec![desired.clone()]);
    };
    if matches.next().is_some()
        || index.get("name") != desired.get("name")
        || !same_keys(
            index.get_document("key").ok(),
            desired.get_document("key").ok(),
        )
        || desired
            .iter()
            .any(|(key, value)| key != "key" && index.get(key) != Some(value))
        || index
            .keys()
            .any(|key| !["v", "ns"].contains(&key.as_str()) && !desired.contains_key(key))
    {
        return Err("Query index key or option drift");
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
    use super::*;
    #[test]
    fn candidate_partial_filter_is_compared_exactly() {
        let index =
            doc! {"name":"pending","key":{"ready":1},"partialFilterExpression":{"pending":true}};
        assert!(strict_missing(std::slice::from_ref(&index), &index)
            .unwrap()
            .is_empty());
        for partial in [None, Some(doc! {"pending":false})] {
            let mut drift = index.clone();
            drift.remove("partialFilterExpression");
            if let Some(partial) = partial {
                drift.insert("partialFilterExpression", partial);
            }
            assert!(strict_missing(&[drift], &index).is_err());
        }
    }
    #[test]
    fn generic_inventory_refuses_drift_and_reconciles_exact_indexes() {
        for index in [
            doc! {"name":"time_desc","key":{"at":-1,"_id":-1}},
            doc! {"name":"owner_state","key":{"owner":1,"state":1}},
        ] {
            assert_eq!(strict_missing(&[], &index).unwrap(), vec![index.clone()]);
            assert!(strict_missing(std::slice::from_ref(&index), &index)
                .unwrap()
                .is_empty());
            for (key, value) in [
                ("unique", mongodb::bson::Bson::Boolean(true)),
                ("hidden", true.into()),
                ("expireAfterSeconds", 5.into()),
                ("collation", doc! {"locale":"en"}.into()),
            ] {
                let mut drift = index.clone();
                drift.insert(key, value);
                assert!(strict_missing(&[drift], &index).is_err());
            }
            assert!(strict_missing(&[index.clone(), index.clone()], &index).is_err());
            assert!(strict_missing(
                &[doc! {"name":"_id_","key":{"_id":1},"collation":{"locale":"en"}}],
                &index
            )
            .is_err());
        }
    }
}
