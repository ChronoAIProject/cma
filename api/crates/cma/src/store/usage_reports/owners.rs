use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    cursor::Cursor, Order, Owner, OwnerQuery, Owners, Report, Sort,
};
impl ReportingStore {
    pub(super) async fn read_owners(&self, report: &Report, query: OwnerQuery) -> Result<Owners> {
        if !report.scope.admin {
            return Err(MetricsError::Forbidden);
        }
        let limit = query.validate()?;
        let _timer = self.time_read("report_owner_search")?;
        // A separate cursor domain prevents table/owner and search-prefix reuse.
        let hash = format!("owner-search:{}:{}", report.report_id, query.search);
        let after = query
            .cursor
            .as_ref()
            .map(|c| {
                self.signer.decode(
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
        let mut cursor = self
            .collection(ROWS)
            .aggregate([
                doc! {"$match":filter},
                doc! {"$group":{"_id":"$owner"}},
                doc! {"$sort":{"_id":1}},
                doc! {"$limit":i64::from(limit)+1},
            ])
            .max_time(DB_DEADLINE)
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
            Some(self.signer.encode(&Cursor {
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
}
