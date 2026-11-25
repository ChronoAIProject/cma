use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    cursor::{Cursor, CursorSigner},
    Order, Owner, OwnerQuery, Owners, Report, Sort,
};

pub fn validate_owner_page(report: &Report, query: &OwnerQuery) -> Result<u16> {
    if !report.scope.admin {
        return Err(MetricsError::Forbidden);
    }
    query.validate()
}

pub async fn read_owner_page(
    rows: Collection<Document>,
    signer: &CursorSigner,
    report: &Report,
    query: OwnerQuery,
    limit: u16,
    db_deadline: Duration,
) -> Result<Owners> {
    // A separate cursor domain prevents table/owner and search-prefix reuse.
    let hash = format!("owner-search:{}:{}", report.report_id, query.search);
    let after = query
        .cursor
        .as_ref()
        .map(|c| {
            signer.decode(
                c,
                &report.scope,
                &report.report_id,
                &hash,
                Sort::Owner,
                Order::Asc,
            )
        })
        .transpose()?;
    let mut owner_filter = Document::new();
    if !query.search.is_empty() {
        let escaped: String = query
            .search
            .chars()
            .flat_map(|c| {
                if ".+*?()|[]{}^$\\".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        owner_filter.insert("$regex", format!("^{escaped}"));
    }
    if let Some(after) = after {
        owner_filter.insert(
            "$gt",
            after
                .last_value
                .as_str()
                .ok_or_else(|| MetricsError::Invalid("Invalid owner cursor".into()))?,
        );
    }
    let mut filter = doc! {"report_id":&report.report_id};
    if !owner_filter.is_empty() {
        filter.insert("owner", owner_filter);
    }
    let mut cursor = rows
        .aggregate([
            doc! {"$match":filter},
            doc! {"$group":{"_id":"$owner"}},
            doc! {"$sort":{"_id":1}},
            doc! {"$limit":i64::from(limit)+1},
        ])
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut items = Vec::new();
    while let Some(d) = cursor.try_next().await.map_err(unavailable)? {
        items.push(Owner {
            subject: d.get_str("_id").map_err(unavailable)?.into(),
            display_name: None,
        });
    }
    let next = if items.len() > usize::from(limit) {
        items.pop();
        let last = &items.last().ok_or(MetricsError::Unavailable)?.subject;
        Some(signer.encode(&Cursor {
            principal: report.scope.principal.clone(),
            admin: true,
            filter_hash: hash,
            report_id: report.report_id.clone(),
            sort: Sort::Owner,
            order: Order::Asc,
            last_value: last.clone().into(),
            last_null: false,
            sandbox_id: String::new(),
        })?)
    } else {
        None
    };
    Ok(Owners {
        items,
        next,
        report_id: report.report_id.clone(),
    })
}
