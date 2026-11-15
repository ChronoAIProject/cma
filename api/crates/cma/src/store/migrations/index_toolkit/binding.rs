use super::super::Result;
use super::catalog::{Binding, Query};
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(super) async fn resolve(database: &Database, query: &Query) -> Result<Option<Document>> {
    let field = match query.binding {
        Binding::Static => return Ok(Some(query.filter.clone())),
        Binding::Equality(field) => field,
        Binding::UsageInterval => "sandbox_id",
    };
    // Sample only the fields needed to bind a real query. They never leave this module.
    let mut predicate = Document::new();
    predicate.insert(field, doc! {"$type":"string"});
    let mut projection = doc! {"_id":0};
    projection.insert(field, 1);
    if matches!(query.binding, Binding::UsageInterval) {
        predicate.insert("started_at", doc! {"$type":["long","int"]});
        projection.insert("started_at", 1);
    }
    let response = database
        .run_command(doc! {
            "find":query.collection,"filter":predicate,"sort":{"_id":1},
            "projection":projection,"limit":1,"batchSize":1,"singleBatch":true,"maxTimeMS":5000
        })
        .await
        .map_err(|_| "representative query value unavailable")?;
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "query sample cursor malformed")?;
    if cursor.get_i64("id").ok() != Some(0) {
        return Err("query sample cursor exceeds bound");
    }
    let rows = cursor
        .get_array("firstBatch")
        .map_err(|_| "query sample rows malformed")?;
    if rows.len() > 1 {
        return Err("query sample exceeds bound");
    }
    rows.first()
        .map(|row| {
            bind(
                query,
                row.as_document().ok_or("query sample malformed")?,
                field,
            )
        })
        .transpose()
}

fn bind(query: &Query, row: &Document, field: &str) -> Result<Document> {
    let value = row
        .get_str(field)
        .map_err(|_| "query sample value malformed")?;
    if value.is_empty() || value.len() > 1024 {
        return Err("query sample value outside bound");
    }
    let mut filter = query.filter.clone();
    if matches!(query.binding, Binding::UsageInterval) {
        let at = super::inspection::number(row, "started_at")?;
        if at < 0 {
            return Err("query sample time outside bound");
        }
        let from = at / 86400 * 86400;
        let to = from
            .checked_add(86400)
            .ok_or("query sample time overflow")?;
        filter.insert(field, doc! {"$in":[value]});
        filter.insert("ended_at", doc! {"$gt":from});
        filter.insert("started_at", doc! {"$lt":to});
    } else {
        filter.insert(field, value);
    }
    Ok(filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interval_uses_one_real_id_and_integer_utc_day_with_safe_failures() {
        let query = super::super::candidates::usage().remove(0);
        assert_eq!(
            bind(
                &query,
                &doc! {"sandbox_id":"sample","started_at":90001_i64},
                "sandbox_id"
            )
            .unwrap(),
            doc! {"sandbox_id":{"$in":["sample"]},"ended_at":{"$gt":86400_i64},"started_at":{"$lt":172800_i64}}
        );
        for row in [
            doc! {"sandbox_id":""},
            doc! {"sandbox_id":"sample","started_at":-1_i64},
            doc! {"sandbox_id":"sample","started_at":i64::MAX},
            doc! {"sandbox_id":"sample","started_at":"wrong"},
        ] {
            assert!(bind(&query, &row, "sandbox_id").is_err());
        }
    }
}
