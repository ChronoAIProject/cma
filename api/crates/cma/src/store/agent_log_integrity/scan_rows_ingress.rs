//! Shared scan bookkeeping and owned conversion of each actual raw row.
use super::*;
use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    row::{self, ReadError},
};
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    OwnedWork, SharedOwned,
};

#[derive(Default)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(in crate::store::agent_log_integrity) struct ScanState {
    entry_count: u64,
    encoded_bytes: usize,
    head_hash: [u8; HASH_BYTES],
}

pub(super) fn require_head(last_seq: u64) -> Result<(), SandboxStoreError> {
    if last_seq > PHYSICAL_AGENT_LOG_ENTRIES {
        tracing::warn!(
            limit_kind = "physical_entries",
            existing_entries = last_seq,
            max_entries = PHYSICAL_AGENT_LOG_ENTRIES,
            "Agent log capacity rejected"
        );
        return Err(capacity_error());
    }
    Ok(())
}

impl ScanState {
    pub(super) fn prepare(
        &mut self,
        sequence: u64,
        last_seq: u64,
        document_bytes: usize,
    ) -> Result<u64, SandboxStoreError> {
        let expected = self.entry_count.checked_add(1);
        if expected != Some(sequence) || sequence > last_seq {
            return Err(SandboxStoreError::SequenceConflict);
        }
        self.encoded_bytes = self
            .encoded_bytes
            .checked_add(document_bytes)
            .filter(|total| *total <= PHYSICAL_AGENT_LOG_BYTES)
            .ok_or_else(|| {
                tracing::warn!(
                    limit_kind = "physical_bytes",
                    scanned_entries = self.entry_count,
                    scanned_bytes = self.encoded_bytes,
                    document_bytes,
                    max_bytes = PHYSICAL_AGENT_LOG_BYTES,
                    "Agent log capacity rejected"
                );
                capacity_error()
            })?;
        Ok(expected.unwrap_or_default())
    }

    pub(super) fn advance(&mut self, expected: u64, raw: &mongodb::bson::RawDocument) {
        self.head_hash = hash_raw(raw.as_bytes());
        self.entry_count = expected;
    }

    pub(super) fn finish(self, last_seq: u64) -> Result<LogSummary, SandboxStoreError> {
        if self.entry_count != last_seq {
            return Err(SandboxStoreError::SequenceConflict);
        }
        Ok(LogSummary {
            entry_count: self.entry_count,
            encoded_bytes: self.encoded_bytes,
            head_hash: self.head_hash,
            semantic: None,
        })
    }
}

/// The caller retains raw/capture custody and cursor provenance. A retained
/// visitor value keeps its Data owner; visitor failures must already be owned.
/// A visitor failure is terminal: discard the partial state, whose byte count
/// intentionally advances before the visitor, as in the existing scanner.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "admitted scan cursor composition pending")
)]
pub(in crate::store::agent_log_integrity) fn consume_row_admitted<'a, F>(
    state: &'a mut ScanState,
    last_seq: u64,
    raw: &'a mongodb::bson::RawDocument,
    policy: &'a IngressOwnershipPolicy,
    visit: F,
) -> Result<AdmittedOperation<'a, Result<(), ReadFailure>>, AdmittedError>
where
    F: FnOnce(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>> + Send + 'a,
{
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked("integrity scan row operation", &[])?,
        move |scope| async move { consume_row_owned(state, last_seq, raw, policy, &scope, visit) },
    )
}

pub(super) fn consume_row_owned<F>(
    state: &mut ScanState,
    last_seq: u64,
    raw: &mongodb::bson::RawDocument,
    policy: &IngressOwnershipPolicy,
    scope: &layer2_core::sandbox::ingress_ownership::IngressWorkScope,
    visit: F,
) -> Result<OwnedWork<Result<(), ReadFailure>>, AdmittedError>
where
    F: FnOnce(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>>,
{
    if let Err(error) = require_head(last_seq) {
        return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
    }
    let entry = match row::decode::<proof::ingress::TailSchema>(raw, policy) {
        Ok(entry) => entry,
        Err(error) => return error::finish(Err(ReadError::Row(error)), scope),
    };
    let expected = match state.prepare(entry.seq, last_seq, raw.as_bytes().len()) {
        Ok(expected) => expected,
        Err(error) => return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error)))),
    };
    if let Err(error) = visit(entry) {
        return Ok(error.map(|error| Err(ReadFailure::Domain(error))));
    }
    state.advance(expected, raw);
    Ok(scope.initialize_owned(|| Ok(())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::agent_log_integrity::proof::summary_tests::policy;
    use layer2_core::sandbox::ingress_ownership::IngressWorkScope;
    use layer2_core::sandbox::AgentLogKind;

    fn entry(seq: u64, bytes: usize) -> AgentLogEntry {
        AgentLogEntry {
            agent_id: AgentId("agt_scan_row_fixture".into()),
            seq,
            appended_at: "2026-01-01T00:00:00Z".into(),
            kind: AgentLogKind::AgentMessage {
                content: "a".repeat(bytes),
            },
        }
    }

    fn raw(entry: &AgentLogEntry) -> RawDocumentBuf {
        RawDocumentBuf::try_from(&log_document(entry).unwrap()).unwrap()
    }

    fn plan(bytes: usize) -> AllocationPlan {
        AllocationPlan::checked("scan row fixture", &[bytes]).unwrap()
    }

    #[test]
    fn structural_boundaries_preserve_capacity_sequence_and_summary() {
        assert!(require_head(PHYSICAL_AGENT_LOG_ENTRIES).is_ok());
        assert_eq!(
            require_head(PHYSICAL_AGENT_LOG_ENTRIES + 1),
            Err(capacity_error())
        );
        assert_eq!(
            ScanState::default().finish(0).unwrap(),
            LogSummary::default()
        );
        assert_eq!(
            ScanState::default().finish(1),
            Err(SandboxStoreError::SequenceConflict)
        );
        for (count, sequence, last) in [
            (0, 0, 1),
            (0, 2, 2),
            (1, 1, 2),
            (0, 1, 0),
            (u64::MAX, 0, u64::MAX),
        ] {
            let mut state = ScanState {
                entry_count: count,
                ..Default::default()
            };
            assert_eq!(
                state.prepare(sequence, last, 1),
                Err(SandboxStoreError::SequenceConflict)
            );
            assert_eq!(state.encoded_bytes, 0);
            assert_eq!(state.entry_count, count);
        }
        let document = raw(&entry(1, 8));
        let mut state = ScanState::default();
        let expected = state.prepare(1, 1, PHYSICAL_AGENT_LOG_BYTES).unwrap();
        assert_eq!(state.entry_count, 0);
        assert_eq!(state.head_hash, [0; HASH_BYTES]);
        state.advance(expected, &document);
        assert_eq!(state.prepare(2, 2, 1), Err(capacity_error()));
        assert_eq!(state.encoded_bytes, PHYSICAL_AGENT_LOG_BYTES);
        let summary = state.finish(1).unwrap();
        assert_eq!(summary.entry_count, 1);
        assert_eq!(summary.encoded_bytes, PHYSICAL_AGENT_LOG_BYTES);
        assert_eq!(summary.head_hash, hash_raw(document.as_bytes()));
        assert!(summary.semantic.is_none());
        let mut overflow = ScanState {
            encoded_bytes: usize::MAX,
            ..Default::default()
        };
        assert_eq!(overflow.prepare(1, 1, 1), Err(capacity_error()));
        assert_eq!(overflow.encoded_bytes, usize::MAX);
    }

    #[tokio::test]
    async fn each_actual_row_keeps_independent_data_after_the_operation_drops() {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let small = entry(1, 32);
        let large = entry(2, 65536);
        let documents = [raw(&small), raw(&large)];
        let mut state = ScanState::default();
        let mut retained = Vec::new();
        for document in &documents {
            let result = consume_row_admitted(&mut state, 2, document, &policy, |value| {
                retained.push(value);
                Ok(())
            })
            .unwrap()
            .await
            .unwrap();
            assert!(result.belongs_to(&policy));
            drop(result.transpose().unwrap());
        }
        assert_eq!(*retained[0], small);
        assert_eq!(*retained[1], large);
        assert!(retained[1].requested_bytes() > retained[0].requested_bytes());
        assert_eq!(
            idle - root.available_bytes(),
            retained.iter().map(|v| v.requested_bytes()).sum::<usize>()
        );
        let summary = state.finish(2).unwrap();
        assert_eq!(
            summary.encoded_bytes,
            documents.iter().map(|d| d.as_bytes().len()).sum::<usize>()
        );
        assert_eq!(summary.head_hash, hash_raw(documents[1].as_bytes()));
        assert!(summary.semantic.is_none());
        let old = retained.remove(0);
        drop(retained);
        assert_eq!(idle - root.available_bytes(), old.requested_bytes());
        assert_eq!(*old, small);
        drop(old);
        assert_eq!(root.available_bytes(), idle);
    }

    #[tokio::test]
    async fn refusal_and_cancellation_do_not_decode_visit_or_change_state() {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let document = raw(&entry(1, 8));
        let mut state = ScanState::default();
        let operation = consume_row_admitted(&mut state, 1, &document, &policy, |_| {
            panic!("cancelled visitor ran")
        })
        .unwrap();
        assert!(root.available_bytes() < idle);
        drop(operation);
        assert_eq!(root.available_bytes(), idle);
        assert_eq!(state, ScanState::default());
        let pressure = policy
            .admit(IngressClass::Data, plan(policy.bounds().data_bytes()))
            .unwrap();
        assert!(matches!(
            consume_row_admitted(&mut state, 1, &document, &policy, |_| {
                panic!("Data-refused visitor ran")
            })
            .unwrap()
            .await,
            Err(AdmittedError::Refused(_))
        ));
        assert_eq!(state, ScanState::default());
        drop(pressure);
        let pressure = policy
            .admit(
                IngressClass::Transformation,
                plan(policy.bounds().transformation_bytes()),
            )
            .unwrap();
        assert!(matches!(
            consume_row_admitted(&mut state, 1, &document, &policy, |_| {
                panic!("operation-refused visitor ran")
            }),
            Err(AdmittedError::Refused(_))
        ));
        assert_eq!(state, ScanState::default());
        drop(pressure);
        let mut nested = doc! {"leaf":true};
        for _ in 0..66 {
            nested = doc! {"nested":nested};
        }
        let unsupported = RawDocumentBuf::try_from(&nested).unwrap();
        assert!(matches!(
            consume_row_admitted(&mut state, 1, &unsupported, &policy, |_| {
                panic!("unsupported visitor ran")
            })
            .unwrap()
            .await,
            Err(AdmittedError::Unsupported(_))
        ));
        assert_eq!(state, ScanState::default());
        assert_eq!(root.available_bytes(), idle);
    }

    #[tokio::test]
    async fn original_decode_and_structural_errors_precede_the_visitor() {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut malformed = log_document(&entry(1, 8)).unwrap();
        malformed.insert("seq", 2_i64);
        let original = read_entry(&malformed).unwrap_err();
        let cases = [
            (
                RawDocumentBuf::try_from(&malformed).unwrap(),
                1,
                0,
                original,
            ),
            (raw(&entry(2, 8)), 2, 0, SandboxStoreError::SequenceConflict),
            (raw(&entry(1, 8)), 0, 0, SandboxStoreError::SequenceConflict),
            (
                raw(&entry(1, 8)),
                PHYSICAL_AGENT_LOG_ENTRIES + 1,
                0,
                capacity_error(),
            ),
            (
                raw(&entry(1, 8)),
                1,
                PHYSICAL_AGENT_LOG_BYTES,
                capacity_error(),
            ),
        ];
        for (document, last, bytes, expected) in cases {
            let mut state = ScanState {
                encoded_bytes: bytes,
                ..Default::default()
            };
            let error = consume_row_admitted(&mut state, last, &document, &policy, |_| {
                panic!("invalid row reached visitor")
            })
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
            assert!(matches!(&*error, ReadFailure::Domain(actual) if *actual == expected));
            assert!(error.belongs_to(&policy));
            assert_eq!(state.entry_count, 0);
            assert_eq!(state.encoded_bytes, bytes);
            assert_eq!(state.head_hash, [0; HASH_BYTES]);
            assert!(root.available_bytes() < idle);
            drop(error);
            assert_eq!(root.available_bytes(), idle);
        }
    }

    #[tokio::test]
    async fn visitor_failure_retains_the_original_error_and_separate_entry() {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let document = raw(&entry(1, 4096));
        let mut state = ScanState::default();
        let mut retained = None;
        let storage = IngressWorkScope::new(
            policy
                .admit(IngressClass::Transformation, plan(8192))
                .unwrap(),
        );
        let original =
            storage.initialize_owned(|| SandboxStoreError::Corrupt("visitor failure".repeat(32)));
        let SandboxStoreError::Corrupt(message) = &*original else {
            panic!("fixture")
        };
        let address = message.as_ptr();
        drop(storage);
        let error = consume_row_admitted(&mut state, 1, &document, &policy, |value| {
            retained = Some(value);
            Err(original)
        })
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap_err();
        assert!(
            matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(message)) if message.as_ptr() == address)
        );
        assert!(error.belongs_to(&policy));
        assert_eq!(state.entry_count, 0);
        assert_eq!(state.head_hash, [0; HASH_BYTES]);
        assert_eq!(state.encoded_bytes, document.as_bytes().len());
        assert_eq!(state.finish(1), Err(SandboxStoreError::SequenceConflict));
        drop(error);
        let retained = retained.unwrap();
        assert_eq!(idle - root.available_bytes(), retained.requested_bytes());
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
    }

    #[tokio::test]
    async fn foreign_visitor_error_custody_is_refused_without_advancing_the_scan() {
        let (root, policy) = policy();
        let (foreign_root, foreign) = super::super::super::proof::summary_tests::policy();
        let (idle, foreign_idle) = (root.available_bytes(), foreign_root.available_bytes());
        let document = raw(&entry(1, 8));
        let mut state = ScanState::default();
        let storage = IngressWorkScope::new(
            foreign
                .admit(IngressClass::Transformation, plan(8192))
                .unwrap(),
        );
        let error =
            storage.initialize_owned(|| SandboxStoreError::Corrupt("foreign visitor".into()));
        drop(storage);
        assert!(matches!(
            consume_row_admitted(&mut state, 1, &document, &policy, |_| Err(error))
                .unwrap()
                .await,
            Err(AdmittedError::Refused(_))
        ));
        assert_eq!(state.entry_count, 0);
        assert_eq!(state.head_hash, [0; HASH_BYTES]);
        assert_eq!(root.available_bytes(), idle);
        assert_eq!(foreign_root.available_bytes(), foreign_idle);
    }
}
