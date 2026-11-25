use super::*;
use mongodb::{
    bson::{self, RawDocumentBuf},
    Collection,
};
use std::time::Duration;
use tokio::time::Instant;

tokio::task_local! {
    static CATALOG_BYTES: std::cell::Cell<(usize, usize)>;
}

pub(in crate::store) fn catalog_response(row: &Document) -> Result<(), &'static str> {
    CATALOG_BYTES
        .try_with(|counter| {
            let (used, limit) = counter.get();
            let size = bson::serialize_to_vec(row)
                .map_err(|_| "Query index inventory malformed")?
                .len();
            let used = used.saturating_add(size);
            counter.set((used, limit));
            if used > limit {
                return Err("Preview index inventory exceeds source budget");
            }
            Ok(())
        })
        .unwrap_or(Ok(()))
}

pub(in crate::store) const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub(in crate::store) const READ_DEADLINE: Duration = Duration::from_secs(2);

pub(in crate::store) struct PreviewBudget {
    remaining: usize,
    deadline: Instant,
    pub reads: usize,
    pub bytes: usize,
}

pub(super) struct PreviewRows {
    source: Collection<Document>,
    filter: Document,
    after: Option<i64>,
    remaining: usize,
}

impl PreviewRows {
    pub fn new(source: Collection<Document>, filter: Document, limit: usize) -> Self {
        Self {
            source,
            filter,
            after: None,
            remaining: limit,
        }
    }

    pub async fn next(
        &mut self,
        session: &mut ClientSession,
        budget: &mut PreviewBudget,
    ) -> Result<Option<RawDocumentBuf>, TransactionLogError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let filter = self.after.map_or_else(
            || self.filter.clone(),
            |after| doc! {"$and":[self.filter.clone(), {"seq":{"$gt":after}}]},
        );
        let row = budget
            .raw(&self.source, filter, Some(doc! {"seq":1}), session)
            .await?;
        self.remaining -= 1;
        if let Some(row) = &row {
            self.after = Some(row.get_i64("seq").map_err(|_| super::super::corrupt())?);
        } else {
            self.remaining = 0;
        }
        Ok(row)
    }
}

impl PreviewBudget {
    pub fn new() -> Self {
        Self {
            remaining: MAX_SOURCE_BYTES,
            deadline: Instant::now() + READ_DEADLINE,
            reads: 0,
            bytes: 0,
        }
    }

    pub fn check(&self) -> Result<(), TransactionLogError> {
        if Instant::now() >= self.deadline || self.remaining < 128 {
            return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
        }
        Ok(())
    }

    pub fn require_bytes(&self, bytes: i64) -> Result<(), TransactionLogError> {
        self.check()?;
        if usize::try_from(bytes)
            .ok()
            .is_none_or(|bytes| bytes > self.remaining.saturating_sub(128))
        {
            return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
        }
        Ok(())
    }

    pub fn charge(&mut self, bytes: usize) -> Result<(), TransactionLogError> {
        self.reads += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or(SandboxStoreError::AgentLogCapacityExceeded)?;
        self.check()
    }

    pub fn projection(&self) -> Result<Document, TransactionLogError> {
        self.check()?;
        let limit = i64::try_from(self.remaining.saturating_sub(128))
            .map_err(|_| super::super::corrupt())?;
        Ok(
            doc! {"_id":0,"preview_size":{"$bsonSize":"$$ROOT"},"preview_row":{"$cond":[{"$lte":[{"$bsonSize":"$$ROOT"},limit]},"$$ROOT",null]}},
        )
    }

    pub fn unpack(&mut self, raw: RawDocumentBuf) -> Result<RawDocumentBuf, TransactionLogError> {
        self.charge(raw.as_bytes().len())?;
        let size = raw
            .get_i32("preview_size")
            .map(i64::from)
            .or_else(|_| raw.get_i64("preview_size"))
            .map_err(|_| super::super::corrupt())?;
        let row = raw
            .get_document("preview_row")
            .map_err(|_| SandboxStoreError::AgentLogCapacityExceeded)?
            .to_owned();
        if size != row.as_bytes().len() as i64 {
            return Err(super::super::corrupt().into());
        }
        Ok(row)
    }

    pub async fn raw(
        &mut self,
        source: &Collection<Document>,
        filter: Document,
        sort: Option<Document>,
        session: &mut ClientSession,
    ) -> Result<Option<RawDocumentBuf>, TransactionLogError> {
        let source = source.clone_with_type::<RawDocumentBuf>();
        let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
        let mut find = source
            .find_one(filter)
            .projection(self.projection()?)
            .read_concern(ReadConcern::snapshot());
        if let Some(sort) = sort {
            find = find.sort(sort);
        }
        let raw = tokio::time::timeout_at(self.deadline, find.session(session))
            .await
            .map_err(|_| SandboxStoreError::AgentLogCapacityExceeded)?
            .map_err(TransactionLogError::Driver)?;
        let result = match raw {
            Some(raw) => {
                read.document(raw.as_bytes().len());
                Some(self.unpack(raw)?)
            }
            None => {
                self.charge(5)?;
                None
            }
        };
        read.complete();
        Ok(result)
    }

    pub async fn document(
        &mut self,
        source: &Collection<Document>,
        filter: Document,
        sort: Option<Document>,
        session: &mut ClientSession,
    ) -> Result<Option<Document>, TransactionLogError> {
        self.raw(source, filter, sort, session)
            .await?
            .map(|raw| Document::try_from(raw).map_err(|_| super::super::corrupt().into()))
            .transpose()
    }

    pub fn charge_document(&mut self, row: &Document) -> Result<(), TransactionLogError> {
        self.charge(
            bson::serialize_to_vec(row)
                .map_err(|_| super::super::corrupt())?
                .len(),
        )
    }

    pub async fn catalog<F: std::future::Future>(
        &mut self,
        read: F,
    ) -> Result<F::Output, TransactionLogError> {
        self.check()?;
        let (result, bytes) = CATALOG_BYTES
            .scope(std::cell::Cell::new((0, self.remaining)), async {
                let result = read.await;
                (result, CATALOG_BYTES.with(|counter| counter.get().0))
            })
            .await;
        self.charge(bytes)?;
        Ok(result)
    }
}
