//! Owner quota fixture shared by domain and public-interface tests.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct MemoryDocumentQuota {
    pub ready: AtomicBool,
    pub unavailable: AtomicBool,
    pub limits: [u64; 3],
    reservations: Mutex<BTreeMap<String, DocumentSourceReservation>>,
}

impl MemoryDocumentQuota {
    pub fn new(limits: [u64; 3]) -> Self {
        Self {
            ready: true.into(),
            unavailable: false.into(),
            limits,
            reservations: Default::default(),
        }
    }

    fn check(&self) -> Result<(), DocumentError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(DocumentError::Unavailable);
        }
        Ok(())
    }

    fn counters(
        &self,
        rows: &BTreeMap<String, DocumentSourceReservation>,
        owner: &str,
    ) -> [u64; 3] {
        let sources: Vec<_> = rows.values().filter(|r| r.owner_hash == owner).collect();
        let documents: std::collections::BTreeSet<_> =
            sources.iter().map(|r| &r.document_id).collect();
        [
            documents.len() as u64,
            sources.iter().map(|r| r.bytes).sum(),
            sources.len() as u64,
        ]
    }
}

#[async_trait::async_trait]
impl DocumentQuotaStore for MemoryDocumentQuota {
    async fn reserve(
        &self,
        source: &DocumentSourceReservation,
    ) -> Result<SourceTicket, DocumentError> {
        source.validate()?;
        self.check()?;
        if !self.ready.load(Ordering::SeqCst) {
            return Err(DocumentError::Unavailable);
        }
        let mut rows = self.reservations.lock().await;
        if let Some(existing) = rows.get(&source.object_key()) {
            return if existing == source {
                Ok(SourceTicket::Ledgered)
            } else {
                Err(DocumentError::Corrupt)
            };
        }
        let usage = self.counters(&rows, &source.owner_hash);
        let new_document = !rows
            .values()
            .any(|r| r.owner_hash == source.owner_hash && r.document_id == source.document_id);
        for (((live, added), limit), dimension) in usage
            .into_iter()
            .zip([u64::from(new_document), source.bytes, 1])
            .zip(self.limits)
            .zip([
                DocumentLimit::Documents,
                DocumentLimit::Bytes,
                DocumentLimit::Objects,
            ])
        {
            if live.checked_add(added).is_none_or(|n| n > limit) {
                return Err(DocumentError::LimitReached {
                    dimension,
                    limit,
                    live,
                });
            }
        }
        rows.insert(source.object_key(), source.clone());
        Ok(SourceTicket::Ledgered)
    }

    async fn usage(&self, owner_hash: &str) -> Result<DocumentUsage, DocumentError> {
        self.check()?;
        if !self.ready.load(Ordering::SeqCst) {
            return Ok(DocumentUsage::not_enforced());
        }
        let usage = self.counters(&*self.reservations.lock().await, owner_hash);
        let [documents, bytes, objects] = [0, 1, 2].map(|i| {
            Some(UsageCounter {
                used: usage[i],
                limit: self.limits[i],
            })
        });
        Ok(DocumentUsage {
            enforcement: QuotaEnforcement::Enforced,
            documents,
            bytes,
            objects,
        })
    }
}
