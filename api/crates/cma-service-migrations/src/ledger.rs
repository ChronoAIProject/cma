//! Durable migration claims and immutable receipts in a service-local collection.
use super::*;

#[derive(Clone)]
pub(super) struct Lease {
    pub(super) manifest: Manifest,
    pub(super) attempt: String,
    pub(super) fence: i64,
}

impl Runner {
    fn unknown_history(&self) -> Document {
        let mut allowed = vec![
            doc! {"_id":"runner","schema":SCHEMA,"kind":"lock","service":self.service},
            doc! {"_id":"activation","schema":SCHEMA,"kind":"activation","service":self.service},
        ];
        for manifest in &self.manifests {
            allowed.push(doc!{"schema":SCHEMA,"service":self.service,"kind":{"$in":["attempt","completion","reconciliation"]},"migration":manifest.id,"checksum":manifest.checksum()});
        }
        doc! {"$nor":allowed}
    }
    pub(super) async fn preflight(&self) -> Result<()> {
        if self
            .rows
            .find_one(self.unknown_history())
            .max_time(IO)
            .await
            .map_err(database_error)?
            .is_some()
        {
            return Err("unknown migration history or checksum; older migrator refused");
        }
        if let Some(lock) = self
            .rows
            .find_one(doc! {"_id":"runner"})
            .max_time(IO)
            .await
            .map_err(database_error)?
        {
            if lock.get_i64("fence").is_err()
                || lock.get_datetime("expires_at").is_err()
                || lock.get_str("holder").is_err()
                || lock.get_bool("pending").is_err()
                || lock.get_bool("strict").is_err()
            {
                return Err("invalid migration lock");
            }
            if lock.get_bool("pending").unwrap_or(false)
                && (lock.get_str("holder").unwrap_or("").is_empty()
                    || lock.get_str("pending_checksum").is_err()
                    || lock.get_str("pending_label").is_err()
                    || lock.get_document("pending_command").is_err())
            {
                return Err("invalid pending effect");
            }
            if lock.get_bool("strict").unwrap_or(false)
                && self
                    .rows
                    .find_one(doc! {"_id":"activation","kind":"activation","service":self.service})
                    .max_time(IO)
                    .await
                    .map_err(database_error)?
                    .is_none()
            {
                return Err("migration activation receipt missing");
            }
            if let Ok(active) = lock.get_str("active") {
                let manifest = self.manifest(active)?;
                if lock.get_str("checksum").ok() != Some(manifest.checksum().as_str()) {
                    return Err("active migration checksum mismatch");
                }
            }
        }
        Ok(())
    }
    pub(super) async fn initialize(&self) -> Result<()> {
        if let Err(error) = self.db.create_collection(self.rows.name()).await {
            if !matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(command) if command.code==48)
            {
                return Err(database_error(error));
            }
        }
        self.rows.update_one(doc!{"_id":"runner"},doc!{"$setOnInsert":{"schema":SCHEMA,"kind":"lock","service":self.service,"fence":0_i64,"holder":"","expires_at":DateTime::from_millis(0),"active":Bson::Null,"checksum":Bson::Null,"pending":false,"strict":false}})
            .upsert(true).await.map_err(database_error)?;
        self.preflight().await
    }
    pub(super) async fn completed(&self, manifest: Manifest) -> Result<bool> {
        let id = format!("completed:{}", manifest.id);
        let Some(row) = self
            .rows
            .find_one(doc! {"_id":&id})
            .max_time(IO)
            .await
            .map_err(database_error)?
        else {
            return Ok(false);
        };
        if row.get_str("checksum").ok() != Some(manifest.checksum().as_str())
            || row.get_str("kind").ok() != Some("completion")
        {
            return Err("migration completion drift");
        }
        let attempt = row
            .get_str("attempt")
            .map_err(|_| "migration completion receipt missing")?;
        if self.rows.find_one(doc!{"_id":attempt,"kind":"attempt","migration":manifest.id,"checksum":manifest.checksum(),"outcome":"completed"}).max_time(IO).await.map_err(database_error)?.is_none() {
            return Err("migration completion receipt missing");
        }
        Ok(true)
    }
    pub(super) async fn claim(
        &self,
        manifest: Manifest,
        release: &Release<'_>,
        legacy: bool,
    ) -> Result<Lease> {
        let attempt = format!("attempt:{}", uuid::Uuid::new_v4().simple());
        self.claim_named(manifest, release, legacy, &attempt).await
    }
    pub(super) async fn claim_named(
        &self,
        manifest: Manifest,
        release: &Release<'_>,
        legacy: bool,
        attempt: &str,
    ) -> Result<Lease> {
        for prerequisite in manifest.prerequisites.iter().filter(|_| !legacy) {
            if !self.completed(self.manifest(prerequisite)?).await? {
                return Err("migration prerequisite is incomplete");
            }
        }
        let mut session = transaction::start(&self.db).await?;
        let result=async {
            if self.rows.find_one(self.unknown_history()).session(&mut session).await.map_err(database_error)?.is_some() {return Err("unknown migration history or checksum; older migrator refused");}
            if !legacy && self.rows.find_one(doc!{"_id":format!("completed:{}",manifest.id)}).session(&mut session).await.map_err(database_error)?.is_some() {return Err("migration already completed; rerun status");}
            let mut filter=doc!{"_id":"runner","schema":SCHEMA,"service":self.service,"pending":false,"$or":[{"active":Bson::Null},{"active":manifest.id,"checksum":manifest.checksum()}],"$expr":{"$lte":["$expires_at","$$NOW"]}};
            if legacy {filter.insert("strict",false);} else {filter.insert("strict",true);}
            let lock=self.rows.find_one_and_update(filter,
                vec![doc!{"$set":{"active":manifest.id,"checksum":manifest.checksum(),"holder":attempt,"fence":{"$add":["$fence",1_i64]},"claimed_at":"$$NOW","expires_at":{"$add":["$$NOW",LEASE_MS]}}}])
                .return_document(ReturnDocument::After).session(&mut session).await.map_err(database_error)?.ok_or("migration lease held or an interrupted effect requires reconciliation")?;
            let fence=lock.get_i64("fence").map_err(|_|"invalid migration fence")?;
            let started=lock.get_datetime("claimed_at").map_err(|_|"invalid migration clock")?;
            // A new holder may resume attempts that stopped before their effect.
            self.rows.update_many(doc!{"kind":"attempt","migration":manifest.id,"outcome":"running","fence":{"$lt":fence}},doc!{"$set":{"outcome":"interrupted"}}).session(&mut session).await.map_err(database_error)?;
            self.rows.insert_one(doc!{"_id":attempt,"schema":SCHEMA,"kind":"attempt","service":self.service,"migration":manifest.id,"checksum":manifest.checksum(),"source":release.source,"image":release.image,"fence":fence,"started_at":*started,"outcome":"running","legacy":legacy,"effects":0_i64}).session(&mut session).await.map_err(database_error)?;
            Ok::<_,&'static str>(Lease{manifest,attempt:attempt.to_owned(),fence})
        }.await;
        match result {
            Ok(lease) => {
                transaction::commit(session)
                    .await
                    .map_err(|_| "migration claim commit unavailable")?;
                Ok(lease)
            }
            Err(reason) => Err(transaction::abort_after(session, "claim", reason).await),
        }
    }
    pub(super) fn lease_filter(&self, lease: &Lease) -> Document {
        doc! {"_id":"runner","schema":SCHEMA,"service":self.service,"active":lease.manifest.id,"checksum":lease.manifest.checksum(),"holder":&lease.attempt,"fence":lease.fence,"$expr":{"$gt":["$expires_at","$$NOW"]}}
    }
    pub(super) async fn renew(&self, lease: &Lease) -> Result<()> {
        let result = self
            .rows
            .update_one(
                self.lease_filter(lease),
                vec![doc! {"$set":{"expires_at":{"$add":["$$NOW",LEASE_MS]}}}],
            )
            .await
            .map_err(database_error)?;
        if result.matched_count != 1 {
            return Err("migration lease lost");
        }
        Ok(())
    }
    pub(super) async fn begin_effect(
        &self,
        lease: &Lease,
        digest: &str,
        label: &str,
        command: &Document,
    ) -> Result<()> {
        let mut filter = self.lease_filter(lease);
        filter.insert("pending", false);
        let result=self.rows.update_one(filter,vec![doc!{"$set":{"pending":true,"pending_checksum":digest,"pending_label":label,"pending_command":{"$literal":command},"dispatched_at":"$$NOW"}}]).await.map_err(database_error)?;
        if result.matched_count != 1 {
            return Err("migration dispatch fence lost");
        }
        Ok(())
    }
    pub(super) async fn end_effect(
        &self,
        lease: &Lease,
        digest: &str,
        outcome: &str,
    ) -> Result<()> {
        let mut session = transaction::start(&self.db).await?;
        let result=async {
            let mut filter=self.lease_filter(lease);filter.insert("pending_checksum",digest);filter.insert("pending",true);
            let modified=self.rows.update_one(filter,doc!{"$set":{"pending":false,"pending_checksum":Bson::Null,"pending_label":Bson::Null,"pending_command":Bson::Null}}).session(&mut session).await.map_err(database_error)?;
            if modified.matched_count!=1{return Err("migration effect acknowledgement fence lost");}
            let modified=self.rows.update_one(doc!{"_id":&lease.attempt,"fence":lease.fence,"outcome":"running"},doc!{"$inc":{"effects":1_i64},"$set":{"last_effect":digest,"last_effect_outcome":outcome}}).session(&mut session).await.map_err(database_error)?;
            if modified.matched_count!=1{return Err("migration attempt changed");}Ok(())
        }.await;
        match result {
            Ok(()) => transaction::commit(session).await,
            Err(reason) => {
                Err(transaction::abort_after(session, "acknowledge effect", reason).await)
            }
        }
    }
    pub(super) async fn finish(&self, lease: &Lease, success: bool, legacy: bool) -> Result<()> {
        let mut session = transaction::start(&self.db).await?;
        let result=async {
            let outcome=if success{"completed"}else{"failed"};
            let mut set=doc!{"expires_at":DateTime::from_millis(0)};
            if success {set.extend(doc!{"holder":"","active":Bson::Null,"checksum":Bson::Null,"pending":false,"pending_checksum":Bson::Null,"pending_label":Bson::Null,"pending_command":Bson::Null});}
            let mut filter=self.lease_filter(lease);if success{filter.insert("pending",false);}
            let lock=self.rows.find_one_and_update(filter,doc!{"$set":set}).session(&mut session).await.map_err(database_error)?.ok_or("migration lease lost")?;
            let modified=self.rows.update_one(doc!{"_id":&lease.attempt,"fence":lease.fence,"outcome":"running"},vec![doc!{"$set":{"outcome":outcome,"finished_at":"$$NOW","effect_dispatched":lock.get_bool("pending").unwrap_or(false)}}]).session(&mut session).await.map_err(database_error)?;
            if modified.matched_count!=1 {return Err("migration attempt changed");}
            if success && !legacy {self.rows.insert_one(doc!{"_id":format!("completed:{}",lease.manifest.id),"schema":SCHEMA,"kind":"completion","service":self.service,"migration":lease.manifest.id,"checksum":lease.manifest.checksum(),"attempt":&lease.attempt,"fence":lease.fence}).session(&mut session).await.map_err(database_error)?;}
            Ok(())
        }.await;
        match result {
            Ok(()) => transaction::commit(session).await,
            Err(reason) => Err(transaction::abort_after(session, "finish", reason).await),
        }
    }
    /// The ledger remains blocked while a previously dispatched effect is uncertain.
    /// A subsequent command can present a drained execution receipt through the
    /// service's operator cutover mechanism, never infer it from a lease timeout.
    pub async fn pending_effect(&self) -> Result<Option<Document>> {
        self.rows.find_one(doc!{"_id":"runner","pending":true}).projection(doc!{"_id":0,"active":1,"checksum":1,"fence":1,"holder":1,"dispatched_at":1,"pending_checksum":1,"pending_label":1,"pending_command":1}).max_time(IO).await.map_err(database_error)
    }
}
