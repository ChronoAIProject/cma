use super::{codec, types::*, PlanStore};
use crate::records::RecordError;
use cmaeg_core::seal::{open_consumer_interaction, open_interaction};

pub struct RecoveryPage {
    pub plans: Vec<AdmittedPlan>,
    /// Corrupt/missing records are surfaced without including their contents.
    pub issues: Vec<RecoveryIssue>,
    /// Resume this bounded scan; no claim that one pass is a consistent snapshot.
    pub next_cursor: Option<String>,
}
pub struct RecoveryIssue {
    pub record_id: String,
    pub error: AdmissionError,
}

impl PlanStore {
    /// Scan at most 32 existing interaction heads. This shares the collection
    /// with other kinds, so an empty plan page is not the end of pagination.
    /// A scheduler must revisit pages; concurrent inserts can sort behind its
    /// cursor. Prepared chunks never appear as committed admissions.
    pub async fn recover_page(
        &self,
        owner: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<RecoveryPage, AdmissionError> {
        if owner.is_empty() || owner.len() > 512 {
            return Err(AdmissionError::Invalid);
        }
        self.scan_page(Some(owner), after, limit).await
    }

    /// Service-owned recovery across tenants. Each opened manifest must match
    /// its stored owner; this does not grant an owner access to another tenant.
    pub async fn recover_all_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<RecoveryPage, AdmissionError> {
        self.scan_page(None, after, limit).await
    }

    async fn scan_page(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<RecoveryPage, AdmissionError> {
        if !(1..=32).contains(&limit) {
            return Err(AdmissionError::Invalid);
        }
        let heads = self
            .records
            .list(owner, after, limit)
            .await
            .map_err(|_| AdmissionError::Unavailable)?;
        if heads.len() > limit
            || heads.iter().any(|head| {
                head.owner.is_empty()
                    || head.owner.len() > 512
                    || owner.is_some_and(|owner| head.owner != owner)
            })
        {
            return Err(AdmissionError::Corrupt);
        }
        let mut previous = after;
        for head in &heads {
            if previous.is_some_and(|previous| head.record_id.as_str() <= previous) {
                return Err(AdmissionError::Corrupt);
            }
            previous = Some(&head.record_id);
        }
        let next_cursor = (heads.len() == limit).then(|| heads.last().unwrap().record_id.clone());
        let mut page = RecoveryPage {
            plans: Vec::new(),
            issues: Vec::new(),
            next_cursor,
        };
        for head in heads {
            let result = self.recover_head(&head.owner, &head.record_id).await;
            match result {
                Ok(Some(plan)) => page.plans.push(plan),
                Ok(None) => {}
                Err(error) => page.issues.push(RecoveryIssue {
                    record_id: head.record_id,
                    error,
                }),
            }
        }
        Ok(page)
    }

    async fn recover_head(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        let url = self
            .records
            .record_url(id)
            .map_err(|_| AdmissionError::Corrupt)?;
        let row = match self.records.get(url.as_str()).await {
            Ok(row) => row,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(AdmissionError::Unavailable),
        };
        // Named-card records use another authenticated token kind. Their
        // revisions are unrelated to immutable routed admission records.
        if row.blob.starts_with(b"cmaeg1.ic.") {
            open_consumer_interaction(&self.keys, id, &row.blob)
                .map_err(|_| AdmissionError::Corrupt)?;
            return Ok(None);
        }
        let opened =
            open_interaction(&self.keys, id, &row.blob).map_err(|_| AdmissionError::Corrupt)?;
        let value: serde_json::Value = codec::decode(&opened.payload)?;
        if value.get("kind").and_then(serde_json::Value::as_str) != Some(ROOT_KIND) {
            return Ok(None);
        }
        let manifest: Manifest =
            serde_json::from_value(value).map_err(|_| AdmissionError::Corrupt)?;
        manifest.validate((self.clock)())?;
        if manifest.identity.owner != owner || manifest.id != id || row.version != 1 {
            return Err(AdmissionError::Corrupt);
        }
        self.restore(&manifest).await.map(Some)
    }
}
