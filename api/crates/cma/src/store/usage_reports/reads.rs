use super::*;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    cursor::Cursor, MetricsStore, Order, PageQuery, Report, ReportQuery, Row, Rows, Scope, Series,
    SeriesQuery,
};

#[async_trait]
impl MetricsStore for ReportingStore {
    async fn options(
        &self,
        scope: &Scope,
        query: layer2_core::sandbox_metrics::OptionQuery,
        owner_context: layer2_core::sandbox_metrics::OwnerContextAccess,
    ) -> Result<layer2_core::sandbox_metrics::FilterOptions> {
        self.read_options(scope, query, owner_context).await
    }
    async fn resolve_agent(&self, scope: &Scope, id: &str) -> Result<String> {
        let mut filter = doc! {"_id":id};
        if !scope.admin {
            filter.insert("owner", &scope.principal);
        }
        let doc = self
            .collection("agents")
            .find_one(filter)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let agent = super::super::agents::read_agent(&doc).map_err(unavailable)?;
        self.authorize_sandbox(scope, agent.sandbox_id.as_str())
            .await?;
        Ok(agent.sandbox_id.to_string())
    }
    async fn authorize_sandbox(&self, scope: &Scope, id: &str) -> Result<()> {
        let mut filter = doc! {"_id":id};
        if !scope.admin {
            filter.insert("owner", &scope.principal);
        }
        self.collection(super::super::sandbox_usage::STATES)
            .find_one(filter)
            .projection(doc! {"_id":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let mut current = self
            .collection("sandboxes")
            .find_one(doc! {"_id":id})
            .projection(doc! {"owner":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if !scope.admin
            && current
                .as_mut()
                .is_some_and(|d| d.get_str("owner").ok() != Some(scope.principal.as_str()))
        {
            return Err(MetricsError::NotFound);
        }
        Ok(())
    }
    async fn submit(&self, scope: Scope, query: ReportQuery, at: u64) -> Result<Report> {
        // Retry a transaction conflict/coalescing race only a bounded number of
        // times. The unique hash and admission transaction remain authoritative.
        for attempt in 0..3 {
            match self.admit(scope.clone(), query.clone(), at).await {
                Err(MetricsError::Unavailable) if attempt < 2 => tokio::task::yield_now().await,
                result => return result,
            }
        }
        Err(MetricsError::Unavailable)
    }
    async fn report(&self, scope: &Scope, id: &str, at: u64) -> Result<Report> {
        let d = self
            .collection(JOBS)
            .find_one(doc! {"_id":id,"principal":&scope.principal,"admin":scope.admin})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let report: Report = decode(&d)?;
        if report.expires_at <= at {
            return Err(MetricsError::Expired);
        }
        if report.status == layer2_core::sandbox_metrics::ReportStatus::Failed {
            return Err(MetricsError::Unavailable);
        }
        Ok(report)
    }
    async fn reauthorize(&self, report: &Report) -> Result<()> {
        // Even admin cached membership must not retain another owner's old
        // attribution after a transfer. Owner changes invalidate the snapshot.
        let pipeline = [
            doc! {"$match":{"report_id":&report.report_id}},
            doc! {"$lookup":{"from":super::super::sandbox_usage::STATES,"localField":"sandbox_id","foreignField":"_id","as":"current"}},
            doc! {"$lookup":{"from":"sandboxes","localField":"sandbox_id","foreignField":"_id","as":"sandbox"}},
            doc! {"$match":{"$expr":{"$or":[
                {"$ne":["$owner",{"$arrayElemAt":["$current.owner",0]}]},
                {"$and":[{"$gt":[{"$size":"$sandbox"},0]},
                    {"$ne":["$owner",{"$arrayElemAt":["$sandbox.owner",0]}]}]}
            ]}}},
            doc! {"$limit":1},
            doc! {"$project":{"_id":1}},
        ];
        let mut cursor = self
            .collection(ROWS)
            .aggregate(pipeline)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if cursor.try_next().await.map_err(unavailable)?.is_some() {
            return Err(MetricsError::NotFound);
        }
        if let Some(id) = &report.query.filters.sandbox_id {
            self.authorize_sandbox(&report.scope, id).await?;
        }
        if let Some(id) = &report.query.filters.agent_id {
            let current = self.resolve_agent(&report.scope, id).await?;
            if report.query.filters.sandbox_id.as_deref() != Some(current.as_str()) {
                return Err(MetricsError::NotFound);
            }
        }
        Ok(())
    }
    async fn page(&self, report: &Report, query: PageQuery) -> Result<Rows> {
        let _timer = self.time_read("report_page")?;
        let limit = query.validate(report.scope.admin)?;
        let hash = layer2_core::sandbox_metrics::query_hash(
            &report.scope,
            &report.query,
            report.as_of / 60,
        )?;
        let mut filter = doc! {"report_id":&report.report_id};
        let value_key = format!("sorts.{}", query.sort.key());
        let null_key = format!("nulls.{}", query.sort.key());
        let direction = if query.order == Order::Asc { 1 } else { -1 };
        let compare = if direction == 1 { "$gt" } else { "$lt" };
        if let Some(token) = &query.cursor {
            let c = self.signer.decode(
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
        let mut cursor = self
            .collection(ROWS)
            .find(filter)
            .sort(doc! {&null_key:1,&value_key:direction,"sandbox_id":direction})
            .limit(i64::from(limit) + 1)
            .max_time(DB_DEADLINE)
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
                self.signer.encode(&Cursor {
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
        Ok(Rows {
            items: docs.iter().map(decode).collect::<Result<_>>()?,
            next,
            report_id: report.report_id.clone(),
            as_of: report.as_of,
            total_count: report.summary.as_ref().map_or(0, |s| s.sandbox_count),
        })
    }
    async fn detail(&self, report: &Report, id: &str) -> Result<Row> {
        let d = self
            .collection(ROWS)
            .find_one(doc! {"report_id":&report.report_id,"sandbox_id":id})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        decode(&d)
    }
    async fn owners(
        &self,
        report: &Report,
        query: layer2_core::sandbox_metrics::OwnerQuery,
    ) -> Result<layer2_core::sandbox_metrics::Owners> {
        self.read_owners(report, query).await
    }
    async fn series(&self, report: &Report, query: SeriesQuery) -> Result<Series> {
        self.read_series(report, query).await
    }
}
