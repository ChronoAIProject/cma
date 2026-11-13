use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    cursor::Cursor, disambiguate_options_with_context, option_label, FilterOption, FilterOptions,
    OptionContext, OptionKind, OptionQuery, Order, OwnerContextAccess, Scope, Sort,
};

const DOMAIN: &str = "usage-options-v1";
impl ReportingStore {
    pub(super) async fn read_options(
        &self,
        scope: &Scope,
        query: OptionQuery,
        owner_context: OwnerContextAccess,
    ) -> Result<FilterOptions> {
        if !scope.admin {
            return Err(MetricsError::Forbidden);
        }
        let limit = query.validate()?;
        let hash = query.cursor_key()?;
        let after = query
            .cursor
            .as_deref()
            .map(|token| {
                self.signer
                    .decode(token, scope, DOMAIN, &hash, Sort::Name, Order::Asc)
            })
            .transpose()?;
        let after = after
            .as_ref()
            .map(|cursor| {
                cursor
                    .last_value
                    .as_str()
                    .ok_or_else(|| MetricsError::Invalid("Invalid option cursor.".into()))
            })
            .transpose()?;
        let _timer = self.time_read("filter_options")?;
        let (collection, pipeline) = option_pipeline(&query, after, limit);
        // Discovery does not submit, poll, or require a report, and does not use
        // report admission. Only a bounded public projection leaves the store.
        // Driver errors can contain query text; never log them or display names.
        let mut cursor = self
            .collection(collection)
            .aggregate(pipeline)
            .max_time(DB_DEADLINE)
            .batch_size(u32::from(limit) + 1)
            .await
            .map_err(|_| MetricsError::Unavailable)?;
        let mut items = Vec::new();
        let mut hints = Vec::new();
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(|_| MetricsError::Unavailable)?
        {
            let value = row.get_str("_id").map_err(|_| MetricsError::Unavailable)?;
            hints.push(match query.kind {
                OptionKind::Owner => OptionContext::owner_with_access(
                    row.get_str("first_seen_at").ok(),
                    owner_context,
                ),
                OptionKind::Sandbox => OptionContext::sandbox(
                    row.get_str("owner").unwrap_or(""),
                    row.get_str("owner_name").ok(),
                    row.get_i64("created_at")
                        .ok()
                        .and_then(|at| u64::try_from(at).ok()),
                ),
                OptionKind::Runtime => OptionContext::default(),
            });
            items.push(FilterOption {
                value: value.into(),
                label: option_label(query.kind, value, row.get_str("name").ok()),
                disambiguator: None,
            });
        }
        let next = if items.len() > usize::from(limit) {
            items.pop();
            Some(
                self.signer.encode(&Cursor {
                    principal: scope.principal.clone(),
                    admin: true,
                    filter_hash: hash,
                    report_id: DOMAIN.into(),
                    sort: Sort::Name,
                    order: Order::Asc,
                    last_value: items
                        .last()
                        .ok_or(MetricsError::Unavailable)?
                        .value
                        .clone()
                        .into(),
                    last_null: false,
                    sandbox_id: String::new(),
                })?,
            )
        } else {
            None
        };
        disambiguate_options_with_context(&mut items, &hints);
        Ok(FilterOptions { items, next })
    }
}

fn option_pipeline(
    query: &OptionQuery,
    after: Option<&str>,
    limit: u16,
) -> (&'static str, Vec<Document>) {
    let mut pipeline = Vec::new();
    let collection = if query.kind == OptionKind::Runtime {
        "sandboxes"
    } else {
        super::super::sandbox_usage::STATES
    };
    match query.kind {
        OptionKind::Owner => {
            // Use the existing owner index before grouping; include retained owners
            // without directory records as well as users with display names.
            let mut filter = Document::new();
            if let Some(value) = &query.selected {
                filter.insert("owner", value);
            } else if let Some(after) = after {
                filter.insert("owner", doc! {"$gt":after});
            }
            pipeline.extend([
                doc! {"$match":filter},
                doc! {"$group":{"_id":"$owner"}},
                doc! {"$lookup":{"from":"users","localField":"_id","foreignField":"_id","as":"identity"}},
                doc! {"$project":{"name":{"$arrayElemAt":["$identity.display_name",0]},"first_seen_at":{"$arrayElemAt":["$identity.first_seen_at",0]}}},
            ]);
        }
        OptionKind::Sandbox => {
            let mut filter = Document::new();
            if let Some(owner) = &query.owner {
                filter.insert("owner", owner);
            }
            if let Some(value) = &query.selected {
                filter.insert("_id", value);
            } else if let Some(after) = after {
                filter.insert("_id", doc! {"$gt":after});
            }
            pipeline.extend([
                doc! {"$match":filter},
                doc! {"$project":{"name":"$record.name","owner":1,"created_at":"$record.created_at"}},
            ]);
        }
        OptionKind::Runtime => {
            pipeline.extend([
                doc! {"$project":{"version":{"$ifNull":["$record.runtime.runtime_version","$record.lifecycle.suspended_runtime.runtime_version"]}}},
                doc! {"$match":{"version":{"$type":"string","$ne":""}}},
                doc! {"$group":{"_id":"$version"}},
                doc! {"$set":{"name":"$_id"}},
            ]);
            if let Some(value) = &query.selected {
                pipeline.push(doc! {"$match":{"_id":value}});
            } else if let Some(after) = after {
                pipeline.push(doc! {"$match":{"_id":{"$gt":after}}});
            }
        }
    }
    if !query.search.trim().is_empty() {
        let escaped: String = query
            .search
            .trim()
            .chars()
            .flat_map(|c| {
                if ".+*?()|[]{}^$\\".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        pipeline.push(doc! {"$match":{"name":{"$regex":escaped,"$options":"i"}}});
    }
    pipeline.extend([doc! {"$sort":{"_id":1}}, doc! {"$limit":i64::from(limit)+1}]);
    if query.kind == OptionKind::Sandbox {
        // Join owner display context only after reducing to the bounded page.
        pipeline.extend([
            doc! {"$lookup":{"from":"users","localField":"owner","foreignField":"_id","as":"identity"}},
            doc! {"$project":{"name":1,"owner":1,"created_at":1,"owner_name":{"$arrayElemAt":["$identity.display_name",0]}}},
        ]);
    }
    (collection, pipeline)
}
