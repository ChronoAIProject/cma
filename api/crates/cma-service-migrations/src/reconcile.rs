//! Explicit operator receipts for activation and uncertain dispatched effects.
use super::*;

impl Runner {
    async fn operator(&self) -> Result<Document> {
        let response = self
            .db
            .run_command(doc! {"connectionStatus":1})
            .await
            .map_err(database_error)?;
        let info = response
            .get_document("authInfo")
            .map_err(|_| "operator identity unavailable")?;
        let users = info
            .get_array("authenticatedUsers")
            .map_err(|_| "operator identity unavailable")?;
        let roles = info
            .get_array("authenticatedUserRoles")
            .map_err(|_| "operator authority unavailable")?;
        let required_role = format!("{}_operator", self.rows.name());
        if users.len() != 1
            || !roles.iter().any(|role| {
                role.as_document().is_some_and(|role| {
                    role.get_str("role").ok() == Some(required_role.as_str())
                        && role.get_str("db").ok() == Some(self.db.name())
                })
            })
        {
            return Err("designated service-local migration operator required");
        }
        users[0]
            .as_document()
            .cloned()
            .ok_or("operator identity unavailable")
    }

    /// The operator attests that all pre-ledger migrators and their server-side
    /// commands have drained, and their DDL execution credentials are fenced.
    pub async fn activate(&self, evidence: &str) -> Result<()> {
        validate_evidence(evidence)?;
        let operator = self.operator().await?;
        self.initialize().await?;
        let mut session = transaction::start(&self.db).await?;
        let result = async {
            let row = self.rows.find_one_and_update(doc! {"_id":"runner","strict":false,"pending":false,"holder":"","active":Bson::Null}, doc! {"$set":{"strict":true},"$inc":{"fence":1_i64}}).session(&mut session).await.map_err(database_error)?.ok_or("an unfinished legacy migration or prior activation exists")?;
            self.rows.insert_one(doc! {"_id":"activation","kind":"activation","schema":SCHEMA,"database":self.db.name(),"service":self.service,"operator":operator,"evidence":evidence,"legacy_fence":row.get_i64("fence").map_err(|_| "invalid migration fence")?}).session(&mut session).await.map_err(database_error)?;
            Ok(())
        }.await;
        match result {
            Ok(()) => transaction::commit(session).await,
            Err(reason) => Err(transaction::abort_after(session, "activate", reason).await),
        }
    }

    /// A receipt addresses only the observed pending command, holder and fence.
    pub async fn reconcile(
        &self,
        manifest: Manifest,
        attempt: &str,
        fence: i64,
        command: &str,
        evidence: &str,
    ) -> Result<()> {
        self.verify_manifest(manifest)?;
        validate_evidence(evidence)?;
        let operator = self.operator().await?;
        self.preflight().await?;
        let mut session = transaction::start(&self.db).await?;
        let result = async {
            let row = self.rows.find_one_and_update(doc! {"_id":"runner","service":self.service,"active":manifest.id,"checksum":manifest.checksum(),"holder":attempt,"fence":fence,"pending":true,"pending_checksum":command,"$expr":{"$lte":["$expires_at","$$NOW"]}}, doc! {"$set":{"pending":false,"pending_checksum":Bson::Null,"pending_label":Bson::Null,"pending_command":Bson::Null,"holder":""},"$inc":{"fence":1_i64}}).session(&mut session).await.map_err(database_error)?.ok_or("pending command or expired fence changed")?;
            let label = row.get_str("pending_label").map_err(|_| "pending command label missing")?;
            self.rows.insert_one(doc! {"_id":format!("reconciled:{attempt}:{fence}"),"kind":"reconciliation","schema":SCHEMA,"database":self.db.name(),"service":self.service,"migration":manifest.id,"checksum":manifest.checksum(),"attempt":attempt,"fence":fence,"command":command,"label":label,"descriptor":row.get_document("pending_command").map_err(|_| "pending command missing")?,"operator":operator,"evidence":evidence}).session(&mut session).await.map_err(database_error)?;
            self.rows.update_one(doc! {"_id":attempt,"kind":"attempt","outcome":"running","fence":fence}, vec![doc! {"$set":{"outcome":"interrupted","finished_at":"$$NOW"}}]).session(&mut session).await.map_err(database_error)?;
            Ok(())
        }.await;
        match result {
            Ok(()) => transaction::commit(session).await,
            Err(reason) => Err(transaction::abort_after(session, "reconcile", reason).await),
        }
    }
}
fn validate_evidence(evidence: &str) -> Result<()> {
    if evidence.trim().is_empty() || evidence.len() > 2048 {
        return Err("a bounded reviewed drain evidence reference is required");
    }
    Ok(())
}
