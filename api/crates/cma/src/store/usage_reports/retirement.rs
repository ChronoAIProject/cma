use super::{work::server_seconds, *};
use mongodb::{bson::Bson, ClientSession};

pub(super) const SLOT: &str = "report_cleanup";
const SWEEP_SECONDS: u64 = 30;
const CHILD_BATCH: i64 = 64;

impl ReportingStore {
    pub(crate) async fn prune_cache(&self, at: u64) -> Result<()> {
        self.check_circuit(at)?;
        let holder = uuid::Uuid::new_v4().simple().to_string();
        if !self.lease(SLOT, &holder, SWEEP_SECONDS).await? {
            return Ok(());
        }
        // On failure every replica waits out this lease before retrying.
        self.prune_one(at, &holder).await?;
        self.release(SLOT, &holder).await
    }

    async fn prune_one(&self, at: u64, holder: &str) -> Result<()> {
        // Older binaries armed a delayed TTL on reserved unprojected parents.
        // Disarm the earliest such parent as part of each bounded maintenance tick.
        self.collection(JOBS)
            .find_one_and_update(
                doc! {"reserved":true,"expires_at":{"$type":"date"}},
                doc! {"$set":{"expires_at":Bson::Null}},
            )
            .sort(doc! {"expires_at":1})
            .projection(doc! {"_id":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;

        let Some(job) = self
            .collection(JOBS)
            .find_one(doc! {"reserved":true,"$or":[
                {"logical_expires_at":{"$lte":at as i64}},
                {"kind":views::VIEW_KIND,"retire_after":{"$lte":at as i64}},
            ]})
            .sort(doc! {"cleanup_at":1,"_id":1})
            .projection(doc! {"_id":1,"kind":1,"state":1,"generation":1,
            aggregate_projections::GENERATION_FIELD:1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
        else {
            return Ok(());
        };
        if !self.retire_expired_report(&job).await? {
            return Ok(());
        }
        let mut fence = retirement_fence(&job)?;
        fence.insert("state", retired_state(&job));
        let id = job.get_str("_id").map_err(unavailable)?;
        let projected = job.contains_key(aggregate_projections::GENERATION_FIELD);
        tx::run(
            &self.client,
            (self, &fence, id, holder),
            |&(store, fence, id, holder), session| {
                Box::pin(async move {
                    store
                        .fence_control_transaction(SLOT, holder, session)
                        .await?;
                    // Conflicts with any concurrent update to this exact parent.
                    let touched = store
                        .collection(JOBS)
                        .update_one(fence.clone(), doc! {"$inc":{"cleanup_revision":1_i64}})
                        .session(&mut *session)
                        .await
                        .map_err(tx::mongo)?;
                    if touched.matched_count != 1 {
                        return Ok(());
                    }

                    let families = [
                        (ROWS, doc! {"report_id":id}),
                        (CHARTS, doc! {"report_id":id}),
                    ];
                    // Keep contributor discovery until no derived projection remains.
                    let projections = [
                        (
                            aggregate_projections::COLLECTION,
                            doc! {"report_id":id,"kind":{"$ne":"contributor"}},
                        ),
                        (
                            aggregate_projections::COLLECTION,
                            doc! {"report_id":id,"kind":"contributor"},
                        ),
                    ];
                    let mut remaining = CHILD_BATCH;
                    for (collection, filter) in families
                        .into_iter()
                        .chain(projections.into_iter().take(if projected { 2 } else { 0 }))
                    {
                        if !store
                            .sweep_children(collection, filter, &mut remaining, session)
                            .await?
                        {
                            return Ok(());
                        }
                    }
                    store
                        .collection(JOBS)
                        .update_one(
                            fence.clone(),
                            doc! {"$set":{
                                "reserved":false,"bytes":0_i64,"expires_at":expires(at + 60),
                            }},
                        )
                        .session(session)
                        .await
                        .map_err(tx::mongo)?;
                    Ok(())
                })
            },
        )
        .await
    }

    async fn sweep_children(
        &self,
        collection: &str,
        filter: Document,
        remaining: &mut i64,
        session: &mut ClientSession,
    ) -> tx::Result<bool> {
        let mut cursor = self
            .collection(collection)
            .find(filter.clone())
            .projection(doc! {"_id":1})
            .limit(*remaining + 1)
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        let mut ids = Vec::new();
        while let Some(child) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(tx::mongo)?
        {
            ids.push(
                child
                    .get("_id")
                    .cloned()
                    .ok_or_else(|| tx::domain(MetricsError::Unavailable))?,
            );
        }
        let saw_all = ids.len() as i64 <= *remaining;
        ids.truncate(*remaining as usize);
        if !ids.is_empty() {
            let count = ids.len() as i64;
            let mut exact = filter;
            exact.insert("_id", doc! {"$in":ids});
            let reply = self.db
                .run_command(doc! {"delete":collection,"deletes":[{"q":exact,"limit":0}],"maxTimeMS":2000_i64})
                .session(&mut *session)
                .await
                .map_err(tx::mongo)?;
            if reply
                .get_array("writeErrors")
                .is_ok_and(|errors| !errors.is_empty())
                || reply.contains_key("writeConcernError")
                || reply
                    .get_i64("n")
                    .or_else(|_| reply.get_i32("n").map(i64::from))
                    .ok()
                    != Some(count)
            {
                return Err(tx::domain(MetricsError::Unavailable));
            }
            *remaining -= count;
        }
        Ok(saw_all)
    }

    pub(super) async fn retire_expired_report(&self, candidate: &Document) -> Result<bool> {
        // An indexed selection supplies candidates only. Match the actual
        // generation, state, projection and server-time deadline before retiring.
        let mut filter = retirement_fence(candidate)?;
        filter.insert(
            "$expr",
            doc! {"$or":[
                {"$lte":["$logical_expires_at",server_seconds()]},
                {"$and":[{"$eq":["$kind",views::VIEW_KIND]},
                    {"$lte":[{"$ifNull":["$retire_after",i64::MAX]},server_seconds()]}]},
            ]},
        );
        let updated = self
            .collection(JOBS)
            .update_one(
                filter,
                vec![doc! {"$set":{
                    "state":retired_state(candidate),"expires_at":Bson::Null,"cleanup_at":"$$NOW",
                }}],
            )
            .await
            .map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }
}

fn retirement_fence(job: &Document) -> Result<Document> {
    Ok(doc! {
        "_id":job.get_str("_id").map_err(unavailable)?,
        "state":job.get_str("state").map_err(unavailable)?,
        "reserved":true,
        "generation":job.get_i64("generation").map_err(unavailable)?,
        "kind":job.get("kind").cloned().unwrap_or_else(|| doc! {"$exists":false}.into()),
        aggregate_projections::GENERATION_FIELD:job
            .get(aggregate_projections::GENERATION_FIELD)
            .cloned()
            .unwrap_or_else(|| doc! {"$exists":false}.into()),
    })
}

fn retired_state(job: &Document) -> &'static str {
    if job.get_str("kind").ok() == Some(views::VIEW_KIND) {
        "view_retired"
    } else {
        "retired"
    }
}
