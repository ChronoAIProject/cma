//! Optional service startup can observe only a positively identified competing
//! attempt. An error after acquiring the caller's own lease is terminal.
use super::*;

impl Runner {
    pub async fn legacy_startup<Fut, I, Inspect>(
        &self,
        manifest: Manifest,
        inspect: I,
        apply: Fut,
    ) -> Result<()>
    where
        Fut: std::future::Future<Output = Result<()>>,
        I: Fn() -> Inspect,
        Inspect: std::future::Future<Output = Result<bool>>,
    {
        self.verify_manifest(manifest)?;
        self.initialize().await?;
        let baseline = self.lock().await?;
        let own_attempt = format!("attempt:{}", uuid::Uuid::new_v4().simple());
        let release = Release {
            source: "legacy",
            image: "legacy",
        };
        match self
            .claim_named(manifest, &release, true, &own_attempt)
            .await
        {
            Ok(lease) => self.execute_legacy(lease, inspect, apply).await,
            Err("migration claim commit unavailable") => Err("migration claim commit unavailable"),
            Err(reason) => {
                // A competing transaction may still be making its claim majority-visible.
                let competitor = tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        // No DDL was dispatched by this caller. Refuse an ambiguous
                        // claim if its own attempt is present, regardless of others.
                        if self
                            .rows
                            .find_one(doc! {"_id":&own_attempt})
                            .max_time(IO)
                            .await
                            .map_err(database_error)?
                            .is_some()
                        {
                            return Err(reason);
                        }
                        if let Some(other) = self
                            .competing_attempt(manifest, &baseline, &own_attempt)
                            .await?
                        {
                            return Ok(other);
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                })
                .await
                .map_err(|_| reason)??;
                self.observe_completed(manifest, &competitor, inspect).await
            }
        }
    }

    async fn lock(&self) -> Result<Document> {
        self.rows
            .find_one(doc! {"_id":"runner","service":self.service})
            .max_time(IO)
            .await
            .map_err(database_error)?
            .ok_or("migration lock unavailable")
    }

    async fn competing_attempt(
        &self,
        manifest: Manifest,
        baseline: &Document,
        own_attempt: &str,
    ) -> Result<Option<Document>> {
        let fence = baseline
            .get_i64("fence")
            .map_err(|_| "invalid migration fence")?;
        let mut filter = doc! {"kind":"attempt","service":self.service,"migration":manifest.id,"checksum":manifest.checksum(),"legacy":true,"_id":{"$ne":own_attempt},"outcome":{"$in":["running","completed"]},"fence":{"$gt":fence}};
        // Include the exact holder already observed at entry. A completed
        // historical attempt at the same fence without a holder is irrelevant.
        if baseline.get_str("active").ok() == Some(manifest.id)
            && baseline.get_str("checksum").ok() == Some(manifest.checksum().as_str())
        {
            if let Ok(holder) = baseline.get_str("holder") {
                if !holder.is_empty() {
                    filter.remove("fence");
                    filter.insert(
                        "$or",
                        vec![
                            doc! {"fence":{"$gt":fence}},
                            doc! {"fence":fence,"_id":holder},
                        ],
                    );
                }
            }
        }
        self.rows
            .find_one(filter)
            .sort(doc! {"fence":-1})
            .max_time(IO)
            .await
            .map_err(database_error)
    }

    async fn observe_completed<I, Inspect>(
        &self,
        manifest: Manifest,
        competitor: &Document,
        inspect: I,
    ) -> Result<()>
    where
        I: Fn() -> Inspect,
        Inspect: std::future::Future<Output = Result<bool>>,
    {
        let attempt = competitor
            .get_str("_id")
            .map_err(|_| "competing attempt identity unavailable")?;
        let fence = competitor
            .get_i64("fence")
            .map_err(|_| "competing attempt fence unavailable")?;
        tokio::time::timeout(Duration::from_secs(10),async {
            loop {
                let row=self.rows.find_one(doc!{"_id":attempt,"kind":"attempt","service":self.service,"migration":manifest.id,"checksum":manifest.checksum(),"legacy":true,"fence":fence}).max_time(IO).await.map_err(database_error)?.ok_or("competing attempt disappeared")?;
                match row.get_str("outcome").ok() {
                    Some("completed") if row.get_bool("effect_dispatched").ok()==Some(false) && row.get_datetime("finished_at").is_ok() => {
                        self.preflight().await?;
                        if !self.completion_consistent(&self.lock().await?,manifest,fence) || !inspect().await? || !self.completion_consistent(&self.lock().await?,manifest,fence) {return Err("competing migration completion unavailable");}
                        return Ok(())
                    }
                    Some("running")=>tokio::time::sleep(Duration::from_millis(100)).await,
                    _=>return Err("competing migration did not complete"),
                }
            }
        }).await.map_err(|_|"competing migration completion timed out")?
    }

    /// A majority-completed attempt is immutable. A later, different manifest may
    /// own the service lock while a startup observer checks the earlier schema.
    fn completion_consistent(&self, lock: &Document, manifest: Manifest, fence: i64) -> bool {
        let Ok(current_fence) = lock.get_i64("fence") else {
            return false;
        };
        if current_fence < fence {
            return false;
        }
        if lock.get("active") == Some(&Bson::Null) {
            return lock.get_str("holder").ok() == Some("")
                && lock.get_bool("pending").ok() == Some(false);
        }
        let Ok(active) = lock.get_str("active") else {
            return false;
        };
        current_fence > fence
            && active != manifest.id
            && self
                .manifest(active)
                .is_ok_and(|next| lock.get_str("checksum").ok() == Some(next.checksum().as_str()))
            && lock
                .get_str("holder")
                .is_ok_and(|holder| !holder.is_empty())
    }
}
