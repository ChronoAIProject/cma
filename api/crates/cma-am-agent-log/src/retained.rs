//! Retained source rows are read and validated through the caller's Mongo session.
use super::{corrupt, digest, legacy, Head, TransactionLogError, MAX_BYTES, MAX_ENTRIES};
use layer2_core::ids::AgentId;
use layer2_core::sandbox::{store::SandboxStoreError, AgentLogEntry, AgentLogKind};
use mongodb::{
    bson::{doc, Binary, Document, RawDocumentBuf},
    options::ReadConcern,
    ClientSession, Collection,
};

pub struct RetainedRows<'a> {
    head: &'a Head,
    start: i64,
    end: i64,
    proof: Option<(&'a Binary, i64)>,
}

impl<'a> RetainedRows<'a> {
    pub fn new(
        head: &'a Head,
        range: std::ops::RangeInclusive<i64>,
        proof: Option<(&'a Binary, i64)>,
    ) -> Result<Self, SandboxStoreError> {
        let (start, end) = (*range.start(), *range.end());
        if start > end || end - start + 1 > MAX_ENTRIES {
            return Err(corrupt());
        }
        Ok(Self {
            head,
            start,
            end,
            proof,
        })
    }

    pub async fn scan(
        self,
        collection: &Collection<RawDocumentBuf>,
        session: &mut ClientSession,
        snapshot: bool,
        schema_version: i64,
        observe_document: &mut (dyn FnMut(usize) + Send),
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<i64, TransactionLogError> {
        let Self {
            head,
            start,
            end,
            proof,
        } = self;
        let find = collection
            .find(doc! {"agent_id":&head.agent_id,"seq":{"$gte":start,"$lte":end}})
            .sort(doc! {"seq":1})
            .hint(mongodb::options::Hint::Name("agent_log_sequence".into()))
            .limit(MAX_ENTRIES + 1)
            // Bound the driver batch by 64 rows and Mongo's 16-MiB reply ceiling.
            // One-row batches make a full active tail require 10,128 round trips.
            .batch_size(64);
        let find = if snapshot {
            find.read_concern(ReadConcern::snapshot())
        } else {
            find
        };
        let mut cursor = find
            .session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?;
        let mut expected = start;
        let mut bytes = 0_i64;
        while cursor
            .advance(session)
            .await
            .map_err(TransactionLogError::Driver)?
        {
            observe_document(cursor.current().as_bytes().len());
            let raw: RawDocumentBuf =
                tracing::debug_span!(target: "cma.agent_inventory", "bson_decode")
                    .in_scope(|| cursor.deserialize_current())
                    .map_err(TransactionLogError::Driver)?;
            let entry = legacy::read_entry(
                &Document::try_from(&raw).map_err(|_| corrupt())?,
                schema_version,
            )?;
            if entry.agent_id != AgentId(head.agent_id.clone())
                || i64::try_from(entry.seq).ok() != Some(expected)
                || expected > end
            {
                return Err(SandboxStoreError::SequenceConflict.into());
            }
            if (expected == 1) != matches!(entry.kind, AgentLogKind::Created { .. }) {
                return Err(corrupt().into());
            }
            let size = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
            if size > 16 * 1024 * 1024 {
                return Err(corrupt().into());
            }
            bytes = bytes
                .checked_add(size)
                .filter(|n| *n <= MAX_BYTES)
                .ok_or_else(corrupt)?;
            if let Some((leaves, first)) = proof {
                let actual =
                    digest::leaf(&head.agent_id, &head.incarnation, expected, raw.as_bytes());
                digest::check_leaf(leaves, expected - first, &actual)?;
            }
            visit(entry)?;
            expected += 1;
        }
        if expected != end + 1 {
            return Err(SandboxStoreError::SequenceConflict.into());
        }
        Ok(bytes)
    }
}
