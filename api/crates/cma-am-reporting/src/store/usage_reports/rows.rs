use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    cursor::{Cursor, CursorSigner},
    Order, PageQuery, Report, Row, Rows,
};

pub async fn read_row_page(
    collection: Collection<Document>,
    signer: &CursorSigner,
    report: &Report,
    query: PageQuery,
    deadline: Duration,
) -> Result<Rows> {
    let limit = query.validate(report.scope.admin)?;
    let hash =
        layer2_core::sandbox_metrics::query_hash(&report.scope, &report.query, report.as_of / 60)?;
    let mut filter = doc! {"report_id":&report.report_id};
    let value_key = format!("sorts.{}", query.sort.key());
    let null_key = format!("nulls.{}", query.sort.key());
    let direction = if query.order == Order::Asc { 1 } else { -1 };
    let compare = if direction == 1 { "$gt" } else { "$lt" };
    if let Some(token) = &query.cursor {
        let c = signer.decode(
            token,
            &report.scope,
            &report.report_id,
            &hash,
            query.sort,
            query.order,
        )?;
        let id_after = doc! {"sandbox_id":{compare:&c.sandbox_id}};
        if c.last_null {
            filter.insert(&null_key, true);
            filter.extend(id_after);
        } else {
            let last = mongodb::bson::serialize_to_bson(&c.last_value).map_err(unavailable)?;
            filter.insert(
                "$or",
                vec![
                    doc! {&null_key:true},
                    doc! {&null_key:false,&value_key:{compare:last.clone()}},
                    doc! {&null_key:false,&value_key:last,"sandbox_id":{compare:&c.sandbox_id}},
                ],
            );
        }
    }
    let mut cursor = collection
        .find(filter)
        .sort(doc! {&null_key:1,&value_key:direction,"sandbox_id":direction})
        .limit(i64::from(limit) + 1)
        .batch_size(u32::from(limit) + 1)
        .max_time(deadline)
        .await
        .map_err(unavailable)?;
    let mut docs = Vec::new();
    while let Some(d) = cursor.try_next().await.map_err(unavailable)? {
        docs.push(d);
    }
    let next = if docs.len() > limit as usize {
        docs.pop();
        let d = docs.last().ok_or(MetricsError::Unavailable)?;
        let last = d
            .get_document("sorts")
            .map_err(unavailable)?
            .get(query.sort.key())
            .ok_or(MetricsError::Unavailable)?
            .clone();
        Some(
            signer.encode(&Cursor {
                principal: report.scope.principal.clone(),
                admin: report.scope.admin,
                filter_hash: hash,
                report_id: report.report_id.clone(),
                sort: query.sort,
                order: query.order,
                last_value: mongodb::bson::deserialize_from_bson(last).map_err(unavailable)?,
                last_null: d
                    .get_document("nulls")
                    .map_err(unavailable)?
                    .get_bool(query.sort.key())
                    .map_err(unavailable)?,
                sandbox_id: d.get_str("sandbox_id").map_err(unavailable)?.into(),
            })?,
        )
    } else {
        None
    };
    let items = docs.iter().map(decode).collect::<Result<_>>()?;
    Ok(Rows {
        items,
        next,
        report_id: report.report_id.clone(),
        as_of: report.as_of,
        total_count: report.summary.as_ref().map_or(0, |s| s.sandbox_count),
    })
}

pub async fn read_row(
    collection: Collection<Document>,
    report: &Report,
    id: &str,
    deadline: Duration,
) -> Result<Row> {
    let d = collection
        .find_one(doc! {"report_id":&report.report_id,"sandbox_id":id})
        .max_time(deadline)
        .await
        .map_err(unavailable)?
        .ok_or(MetricsError::NotFound)?;
    decode(&d)
}
