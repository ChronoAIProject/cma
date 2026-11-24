//! Gate transitions conflict on the parent and preserve the usage watermark.
//! No startup initialization, DDL, source deletion or clearing transition.
use super::{
    durability::majority_collection,
    sandbox_mongo::{commit_transaction, record_bson, signed, start_transaction, CommitOutcome},
    sandboxes::read_sandbox,
};
use crate::retirement_archive::{Request, Result, REFUSED};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::{
    retirement_archive::{RetirementArchiveReceipt, RetirementArchiveRequirement},
    Sandbox, SandboxStatus,
};
use layer3_adapter::permanent_custody::{digest, ObjectReceipt};
use mongodb::{
    bson::{doc, Bson, Document},
    Client, ClientSession, Collection, Database,
};

mod census;
mod census_catalog;
mod completion;
mod export;
mod schema;
pub(crate) use export::capture;
#[cfg(test)]
pub(crate) use schema::manifest_collections;
pub(crate) use schema::{matches_current, schema_sha256, validate_manifest_census};
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(super) use tests::{prepare_source, verify_legal_consent_census};
const ORIGINAL: &str = "retirement_archive_original_v1";
const MANIFEST: &str = "retirement_archive_manifest_v1";
const RESTORE: &str = "retirement_archive_restore_v1";

pub(crate) struct ArchiveStore {
    pub client: Client,
    pub database: Database,
}
impl ArchiveStore {
    fn parents(&self) -> Collection<Document> {
        majority_collection(&self.database, "sandboxes")
    }
    pub(crate) async fn inspect(&self, id: &str) -> Result<Document> {
        self.parents()
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| REFUSED)?
            .ok_or(REFUSED)
    }
    pub(crate) async fn original(&self, request: &Request, bytes: &[u8]) -> Result<Vec<u8>> {
        let current = self.inspect(&request.sandbox_id).await?;
        if current.get(ORIGINAL).is_none() {
            let source = mongodb::bson::serialize_to_vec(&current).map_err(|_| REFUSED)?;
            let parent = read_sandbox(&current).map_err(|_| REFUSED)?;
            if digest(&source) == request.source_sha256
                && parent.revision == request.expected_revision
                && parent.status == SandboxStatus::Retiring
                && parent.lifecycle.retirement_archive.is_none()
            {
                return Ok(source);
            }
            return Err(REFUSED);
        }
        accepted(&current, request, bytes)?;
        match current
            .get(ORIGINAL)
            .and_then(|v| v.as_document())
            .and_then(|v| v.get("source"))
        {
            Some(Bson::Binary(b)) => Ok(b.bytes.clone()),
            _ => Err(REFUSED),
        }
    }
    pub(crate) async fn admit(&self, request: &Request, bytes: &[u8]) -> Result<()> {
        request.validate()?;
        let mut session = self.client.start_session().await.map_err(|_| REFUSED)?;
        start_transaction(&mut session).await.map_err(|_| REFUSED)?;
        let result = self.admit_in(&mut session, request, bytes).await;
        if result.is_err() {
            let _ = session.abort_guarded().await;
            return result;
        }
        self.commit_or_reconcile(session, request, bytes, None)
            .await
    }
    async fn admit_in(
        &self,
        session: &mut ClientSession,
        request: &Request,
        bytes: &[u8],
    ) -> Result<()> {
        let current = self
            .parents()
            .find_one(doc! {"_id":&request.sandbox_id})
            .session(&mut *session)
            .await
            .map_err(|_| REFUSED)?
            .ok_or(REFUSED)?;
        let mut parent = read_sandbox(&current).map_err(|_| REFUSED)?;
        if parent.lifecycle.retirement_archive.is_some() {
            accepted(&current, request, bytes)?;
            return Ok(());
        }
        let source = mongodb::bson::serialize_to_vec(&current).map_err(|_| REFUSED)?;
        let now = layer2_core::now_rfc3339();
        if source.len() > 1024 * 1024
            || digest(&source) != request.source_sha256
            || parent.revision != request.expected_revision
            || parent.status != SandboxStatus::Retiring
            || parent
                .work
                .lease
                .as_ref()
                .is_some_and(|l| l.expires_at > now)
            || parent
                .lifecycle
                .home
                .as_ref()
                .and_then(|h| h.attachment())
                .and_then(|h| h.identity().ok())
                .as_deref()
                != Some(&request.home_identity)
            || super::channel_storage_retention::has_marker(&current)
        {
            return Err(REFUSED);
        }
        if super::channel_storage_retention::required(
            &self.database,
            &request.sandbox_id,
            Some(&current),
            Some(&mut *session),
        )
        .await
        .map_err(|_| REFUSED)?
        {
            return Err(REFUSED);
        }
        // The retained intent must be an actual event of this original parent,
        // independently selected by the release owner, never a new delete call.
        let intent=self.database.collection::<Document>("sandbox_events")
            .find_one(doc!{"sandbox_id":&request.sandbox_id,"record.kind.type":"status_changed","record.kind.to":"retiring"})
            .sort(doc!{"seq":-1}).session(&mut *session).await.map_err(|_|REFUSED)?.ok_or(REFUSED)?;
        let event = super::sandbox_events::read_event(&intent).map_err(|_| REFUSED)?;
        if event.sandbox_id.as_str() != request.sandbox_id {
            return Err(REFUSED);
        }
        if digest(&mongodb::bson::serialize_to_vec(&intent).map_err(|_| REFUSED)?)
            != request.retirement_intent_sha256
        {
            return Err(REFUSED);
        }
        parent.lifecycle.retirement_archive = Some(RetirementArchiveRequirement {
            format: 1,
            operation_id: request.operation_id.clone(),
            plan_sha256: digest(bytes),
            source_sha256: request.source_sha256.clone(),
            exclusion_sha256: request.exclusion_sha256.clone(),
            verified: None,
        });
        let original = doc! {"request":binary(bytes),"source":binary(&source),"intent":binary(&mongodb::bson::serialize_to_vec(&intent).map_err(|_|REFUSED)?)};
        self.advance(session, &current, parent, doc! {ORIGINAL:original})
            .await
    }
    pub(crate) async fn retain_manifest(
        &self,
        request: &Request,
        bytes: &[u8],
        receipt: &ObjectReceipt,
    ) -> Result<()> {
        self.put_reference(request, bytes, MANIFEST, receipt).await
    }
    pub(crate) async fn retain_restore(
        &self,
        request: &Request,
        bytes: &[u8],
        receipt: &ObjectReceipt,
    ) -> Result<()> {
        self.put_reference(request, bytes, RESTORE, receipt).await
    }
    async fn put_reference(
        &self,
        request: &Request,
        bytes: &[u8],
        field: &str,
        receipt: &ObjectReceipt,
    ) -> Result<()> {
        let current = self.inspect(&request.sandbox_id).await?;
        let parent = accepted(&current, request, bytes)?;
        if parent
            .lifecycle
            .retirement_archive
            .as_ref()
            .ok_or(REFUSED)?
            .verified
            .is_some()
        {
            return Err(REFUSED);
        }
        let value = record_bson(receipt).map_err(|_| REFUSED)?;
        if let Some(existing) = current.get(field) {
            return if existing == &value {
                Ok(())
            } else {
                Err(REFUSED)
            };
        }
        let mut filter = doc! {"_id":&request.sandbox_id,"revision":signed(parent.revision,"archive revision").map_err(|_|REFUSED)?};
        filter.insert(field, doc! {"$exists":false});
        let mut fields = Document::new();
        fields.insert(field, value.clone());
        let outcome = self
            .parents()
            .update_one(filter, doc! {"$set":fields})
            .await;
        if outcome.is_ok_and(|r| r.matched_count == 1) {
            return Ok(());
        }
        let current = self.inspect(&request.sandbox_id).await?;
        accepted(&current, request, bytes)?;
        if current.get(field) == Some(&value) {
            Ok(())
        } else {
            Err(REFUSED)
        }
    }
    pub(crate) async fn manifest(&self, request: &Request, bytes: &[u8]) -> Result<ObjectReceipt> {
        self.reference(request, bytes, MANIFEST).await
    }
    pub(crate) async fn restore(&self, request: &Request, bytes: &[u8]) -> Result<ObjectReceipt> {
        self.reference(request, bytes, RESTORE).await
    }
    async fn reference(
        &self,
        request: &Request,
        bytes: &[u8],
        field: &str,
    ) -> Result<ObjectReceipt> {
        let document = self.inspect(&request.sandbox_id).await?;
        accepted(&document, request, bytes)?;
        mongodb::bson::deserialize_from_bson(document.get(field).ok_or(REFUSED)?.clone())
            .map_err(|_| REFUSED)
    }
    async fn advance(
        &self,
        session: &mut ClientSession,
        current: &Document,
        mut next: Sandbox,
        mut fields: Document,
    ) -> Result<()> {
        if current.contains_key("usage_capture_fence_v1") {
            return Err(REFUSED);
        }
        let revision = next.revision;
        next.revision = revision.checked_add(1).ok_or(REFUSED)?;
        next.updated_at = layer2_core::now_rfc3339();
        next.validate().map_err(|_| REFUSED)?;
        if fields.contains_key(ORIGINAL) {
            fields.insert(
                "retirement_archive_admitted_sha256",
                digest(&mongodb::bson::serialize_to_vec(&next).map_err(|_| REFUSED)?),
            );
        }
        fields.insert("record", record_bson(&next).map_err(|_| REFUSED)?);
        fields.insert(
            "revision",
            signed(next.revision, "archive revision").map_err(|_| REFUSED)?,
        );
        fields.insert("updated_at", &next.updated_at);
        let changed=self.parents().update_one(doc!{"_id":next.id.as_str(),"revision":signed(revision,"archive revision").map_err(|_|REFUSED)?},doc!{"$set":fields,"$inc":{"runtime_writer_claim_fence":1_i64}})
            .session(&mut *session).await.map_err(|_|REFUSED)?;
        if changed.matched_count != 1 {
            return Err(REFUSED);
        }
        let sequence = u64::try_from(current.get_i64("last_seq").map_err(|_| REFUSED)?)
            .map_err(|_| REFUSED)?;
        super::sandbox_usage::record_collections(
            (
                &majority_collection(&self.database, super::sandbox_usage::STATES),
                &majority_collection(&self.database, super::sandbox_usage::EVENTS),
            ),
            session,
            &next,
            sequence,
            &next.updated_at,
            (sequence, revision),
            super::sandbox_usage::UsageObservation::Lifecycle,
        )
        .await
        .map(|_| ())
        .map_err(|_| REFUSED)
    }
    async fn commit_or_reconcile(
        &self,
        mut session: ClientSession,
        request: &Request,
        bytes: &[u8],
        verified: Option<&RetirementArchiveReceipt>,
    ) -> Result<()> {
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Unknown(_) => {
                let at = session.operation_time().ok_or(REFUSED)?;
                let store = Self {
                    client: self.client.clone(),
                    database: self.database.clone(),
                };
                let request = request.clone();
                let bytes = bytes.to_vec();
                let verified = verified.cloned();
                super::sandbox_mongo::wait_for_reconciliation(
                    "retirement archive commit",
                    async move {
                        // An unpinned majority read can precede a committed write
                        // whose acknowledgement was lost. Use its commit-time
                        // snapshot, with the shared bounded reconciliation budget.
                        let mut session = store
                            .client
                            .start_session()
                            .snapshot(true)
                            .snapshot_time(at)
                            .await
                            .map_err(|_| REFUSED)?;
                        let current = store
                            .parents()
                            .find_one(doc! {"_id":&request.sandbox_id})
                            .read_concern(mongodb::options::ReadConcern::snapshot())
                            .session(&mut session)
                            .await
                            .map_err(|_| REFUSED)?
                            .ok_or(REFUSED)?;
                        let parent = accepted(&current, &request, &bytes)?;
                        if verified.is_none()
                            || parent
                                .lifecycle
                                .retirement_archive
                                .as_ref()
                                .and_then(|g| g.verified.as_ref())
                                == verified.as_ref()
                        {
                            Ok(())
                        } else {
                            Err(REFUSED)
                        }
                    },
                )
                .await
                .map_err(|_| REFUSED)?
            }
            CommitOutcome::Retry(_) | CommitOutcome::Failed(_) => Err(REFUSED),
        }
    }
}
pub(crate) fn accepted(document: &Document, request: &Request, bytes: &[u8]) -> Result<Sandbox> {
    let parent = read_sandbox(document).map_err(|_| REFUSED)?;
    let gate = parent
        .lifecycle
        .retirement_archive
        .as_ref()
        .ok_or(REFUSED)?;
    let original = document.get_document(ORIGINAL).map_err(|_| REFUSED)?;
    if gate.operation_id != request.operation_id
        || gate.plan_sha256 != digest(bytes)
        || gate.source_sha256 != request.source_sha256
        || gate.exclusion_sha256 != request.exclusion_sha256
        || parent.id.as_str() != request.sandbox_id
        || original.get("request") != Some(&binary(bytes))
    {
        return Err(REFUSED);
    }
    if parent.retirement_archive_pending() {
        let admitted = document
            .get_str("retirement_archive_admitted_sha256")
            .map_err(|_| REFUSED)?;
        let record = document.get_document("record").map_err(|_| REFUSED)?;
        if parent.revision != request.expected_revision.checked_add(1).ok_or(REFUSED)?
            || digest(&mongodb::bson::serialize_to_vec(record).map_err(|_| REFUSED)?) != admitted
        {
            return Err(REFUSED);
        }
    }
    Ok(parent)
}
fn binary(bytes: &[u8]) -> Bson {
    Bson::Binary(mongodb::bson::Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Generic,
        bytes: bytes.to_vec(),
    })
}

/// Old checkpoint tombstones retain only the content key. Resolve its existing
/// hashed parent prefix; no new ownership inference or schema rewrite is needed.
pub(super) async fn holds_checkpoint_key(database: &Database, key: &str) -> Result<bool> {
    use futures_util::TryStreamExt;
    let mut cursor = majority_collection(database, "sandboxes")
        .find(doc! {"record.lifecycle.retirement_archive":{"$exists":true}})
        .max_time(std::time::Duration::from_secs(10))
        .limit(1001)
        .await
        .map_err(|_| REFUSED)?;
    let mut examined = 0;
    while let Some(document) = cursor.try_next().await.map_err(|_| REFUSED)? {
        examined += 1;
        if examined > 1000 {
            return Err(REFUSED);
        }
        let sandbox = read_sandbox(&document).map_err(|_| REFUSED)?;
        if sandbox.retirement_archive_pending()
            && key.starts_with(&format!(
                "cma/checkpoints/{}/",
                digest(sandbox.id.as_str().as_bytes())
            ))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
