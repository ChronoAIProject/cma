use std::{ffi::CString, io::Write, path::PathBuf};

use layer3_adapter::resource_execution_protocol::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::runner::Cell;

#[path = "journal_storage.rs"]
mod storage;

pub(super) const MAX_RECORDS: usize = 32;
pub(super) const MAX_JOURNAL_BYTES: usize = 2 * 1024 * 1024;
const MARKER_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Assignment {
    pub instance: OpaqueId,
    pub incarnation: Digest,
}
impl Assignment {
    pub(super) fn matches(&self, receiver: &ReceiverKey) -> bool {
        receiver.kind == ReceiverKind::Runtime
            && receiver.instance == self.instance
            && receiver.incarnation == self.incarnation
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub execution: ExecutionKey,
    pub challenge: ReceiverChallengeV1,
    pub ticket: Option<ExecutionTicketV1>,
    pub cell: Option<Cell>,
    pub continuation: Option<ContinuationV1>,
    pub start: Option<StartObservationV1>,
    pub barrier: Option<BarrierReceiptV1>,
    pub stop: Option<StopReceiptV1>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Floor {
    pub family: GrantFamilyKey,
    pub watermark: Generation,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub version: u32,
    pub assignment: Assignment,
    pub policy_epoch: Generation,
    pub sequence: Generation,
    pub records: Vec<Record>,
    pub floors: Vec<Floor>,
}

impl Snapshot {
    pub(super) fn next(&mut self) -> ProtocolResult<Generation> {
        self.sequence = self.sequence.successor()?;
        Ok(self.sequence)
    }

    fn validate(&self) -> ProtocolResult<()> {
        version(self.version)?;
        if self.records.len() > MAX_RECORDS || self.floors.len() > MAX_RECORDS {
            return Err(ProtocolError::Capacity);
        }
        for (index, record) in self.records.iter().enumerate() {
            if !self.assignment.matches(&record.execution.receiver)
                || record.challenge.receiver != record.execution.receiver
                || self.records[..index]
                    .iter()
                    .any(|other| other.execution == record.execution)
            {
                return Err(ProtocolError::EvidenceMismatch);
            }
            record.challenge.validate()?;
            match (&record.ticket, &record.cell) {
                (Some(ticket), Some(cell)) => {
                    ticket.validate()?;
                    if ticket.execution != record.execution || cell.execution != record.execution {
                        return Err(ProtocolError::EvidenceMismatch);
                    }
                    if let Some(continuation) = &record.continuation {
                        continuation.validate_for(ticket)?;
                    }
                    if let Some(start) = &record.start {
                        start.validate_for(ticket)?;
                        if start.sequence > self.sequence {
                            return Err(ProtocolError::EvidenceMismatch);
                        }
                    }
                }
                (None, None) if record.start.is_none() && record.continuation.is_none() => (),
                _ => return Err(ProtocolError::EvidenceMismatch),
            }
            if let Some(barrier) = &record.barrier {
                barrier.validate_for(&barrier.request)?;
                if barrier.request.member
                    != (MemberKey::Execution {
                        execution: record.execution.clone(),
                    })
                    || barrier.barrier_sequence > self.sequence
                    || record.ticket.as_ref().is_some_and(|ticket| {
                        ticket.registration != barrier.request.registration
                            || ticket.basis.family() != Some(&barrier.request.revocation.family)
                    })
                    || !self.floors.iter().any(|floor| {
                        floor.family == barrier.request.revocation.family
                            && floor.watermark >= barrier.durable_watermark
                    })
                {
                    return Err(ProtocolError::EvidenceMismatch);
                }
            }
            if let Some(stop) = &record.stop {
                let barrier = record
                    .barrier
                    .as_ref()
                    .ok_or(ProtocolError::EvidenceMismatch)?;
                stop.validate_for(&barrier.request)?;
                if stop.barrier != *barrier
                    || stop.stop_sequence > self.sequence
                    || (stop.evidence == StopEvidenceKind::RejectedBeforeStart
                        && record
                            .start
                            .as_ref()
                            .is_some_and(|start| start.state == NativeStartState::Entered))
                {
                    return Err(ProtocolError::EvidenceMismatch);
                }
            }
        }
        for (index, floor) in self.floors.iter().enumerate() {
            if self.floors[..index]
                .iter()
                .any(|other| other.family == floor.family)
            {
                return Err(ProtocolError::EvidenceMismatch);
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    version: u32,
    assignment: Assignment,
    policy_epoch: Generation,
}

pub(super) struct Journal {
    storage: storage::Storage,
    expected: [u8; 32],
    activation_expected: [u8; 32],
    #[cfg(test)]
    pub fail_after: Option<Stage>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Write,
    FileSync,
    Rename,
    DirectorySync,
}

impl Journal {
    pub(super) fn load(
        root: PathBuf,
        assignment: &Assignment,
    ) -> ProtocolResult<Option<(Self, Snapshot)>> {
        let marker = root.with_extension("activated");
        match std::fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return match std::fs::symlink_metadata(marker) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    _ => Err(ProtocolError::Unknown),
                };
            }
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            _ => return Err(ProtocolError::Unknown),
        }
        let storage = storage::Storage::open(&root)?;
        let activation_bytes = storage.marker()?;
        let activation: Activation =
            serde_json::from_slice(&activation_bytes).map_err(|_| ProtocolError::Unknown)?;
        if activation.version != MARKER_VERSION || &activation.assignment != assignment {
            return Err(ProtocolError::Unknown);
        }
        let bytes = storage.state(MAX_JOURNAL_BYTES)?;
        let snapshot: Snapshot =
            serde_json::from_slice(&bytes).map_err(|_| ProtocolError::Unknown)?;
        snapshot.validate()?;
        if snapshot.assignment != *assignment || snapshot.policy_epoch != activation.policy_epoch {
            return Err(ProtocolError::Unknown);
        }
        storage.verify()?;
        Ok(Some((
            Self {
                storage,
                expected: Sha256::digest(&bytes).into(),
                activation_expected: Sha256::digest(&activation_bytes).into(),
                #[cfg(test)]
                fail_after: None,
            },
            snapshot,
        )))
    }

    pub(super) fn verify(&self) -> ProtocolResult<()> {
        self.storage.verify()?;
        let actual: [u8; 32] = Sha256::digest(self.storage.state(MAX_JOURNAL_BYTES)?).into();
        if actual != self.expected {
            return Err(ProtocolError::Unknown);
        }
        // Loss/corruption of the independent latch can never select legacy.
        let marker: [u8; 32] = Sha256::digest(self.storage.marker()?).into();
        if marker != self.activation_expected {
            return Err(ProtocolError::Unknown);
        }
        self.storage.verify()
    }

    pub(super) fn save(&mut self, snapshot: &Snapshot) -> ProtocolResult<()> {
        self.verify()?;
        snapshot.validate()?;
        let mut writer = Encoding(Vec::new());
        serde_json::to_writer(&mut writer, snapshot).map_err(|_| ProtocolError::Capacity)?;
        let bytes = writer.0;
        let temporary = CString::new(format!("{}.tmp", uuid::Uuid::new_v4().simple()))
            .map_err(|_| ProtocolError::Unknown)?;
        let result = (|| {
            let mut file = self.storage.create(&temporary)?;
            file.write_all(&bytes)
                .map_err(|_| ProtocolError::Unavailable)?;
            self.boundary(Stage::Write)?;
            file.sync_all().map_err(|_| ProtocolError::Unavailable)?;
            self.boundary(Stage::FileSync)?;
            self.storage.publish(&temporary)?;
            self.boundary(Stage::Rename)?;
            self.storage.sync()?;
            self.boundary(Stage::DirectorySync)?;
            self.expected = Sha256::digest(&bytes).into();
            Ok(())
        })();
        if result.is_err() {
            self.storage.remove_temporary(&temporary);
        }
        result
    }

    fn boundary(&self, _stage: Stage) -> ProtocolResult<()> {
        #[cfg(test)]
        if self.fail_after == Some(_stage) {
            return Err(ProtocolError::Unavailable);
        }
        Ok(())
    }

    /// R3 owns the production activation barrier. Fixtures create already
    /// activated state; the environment flag alone never creates these files.
    #[cfg(test)]
    pub(super) fn activate_fixture(root: &std::path::Path, snapshot: &Snapshot) {
        snapshot.validate().unwrap();
        crate::fs_durability::create_dir_all(root).unwrap();
        let activation = Activation {
            version: MARKER_VERSION,
            assignment: snapshot.assignment.clone(),
            policy_epoch: snapshot.policy_epoch,
        };
        crate::fs_durability::write(
            &root.with_extension("activated"),
            &serde_json::to_vec(&activation).unwrap(),
        )
        .unwrap();
        crate::fs_durability::write(&root.join("writer.lock"), b"").unwrap();
        crate::fs_durability::write(
            &root.join("state.json"),
            &serde_json::to_vec(snapshot).unwrap(),
        )
        .unwrap();
    }
}

struct Encoding(Vec<u8>);
impl Write for Encoding {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_JOURNAL_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("resource journal capacity"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
