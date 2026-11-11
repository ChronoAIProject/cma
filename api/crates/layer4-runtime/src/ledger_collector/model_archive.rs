//! Contract double for addendum 2. Synthetic already-validated artifacts stand
//! at A's API boundary; this does not qualify A's parser, SQLite or physical hold.
use super::*;
use sha2::{Digest as _, Sha256};

pub(super) fn fingerprint(group: Digest, originals: &[Vec<u8>; 3]) -> Digest {
    let key = group.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut hash = Sha256::new();
    hash.update(b"chrono.runtime.ledger.collection.artifacts.v1\0");
    for bytes in std::iter::once(key.as_bytes()).chain(originals.iter().map(Vec::as_slice)) {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    hash.finalize().into()
}

#[test]
fn model_bundle_hash_frames_each_original_part_and_the_selected_group() {
    let first = [b"a".to_vec(), b"bc".to_vec(), b"d".to_vec()];
    let second = [b"ab".to_vec(), b"c".to_vec(), b"d".to_vec()];
    assert_ne!(
        fingerprint(digest(1), &first),
        fingerprint(digest(1), &second)
    );
    assert_ne!(
        fingerprint(digest(1), &first),
        fingerprint(digest(2), &first)
    );
}

pub(super) fn prepared(
    checkpoint: &Checkpoint,
    group: Digest,
    operation: Operation,
    decision: Decision,
) -> (Intent, [Vec<u8>; 3]) {
    let originals = [
        format!(
            "model candidate {:?} at {}",
            group, checkpoint.source_revision
        )
        .into_bytes(),
        format!("model request {operation:?} for {group:?}").into_bytes(),
        format!(
            "model observation {decision:?} at {}",
            checkpoint.source_revision
        )
        .into_bytes(),
    ];
    let intent = Intent {
        operation,
        binding: checkpoint.binding,
        group,
        before_revision: checkpoint.source_revision,
        plan_sha256: digest(3), // C plan, deliberately distinct from evidence.
        source_sha256: digest(checkpoint.source_revision),
        observation_sha256: Sha256::digest(&originals[2]).into(),
        evidence_sha256: fingerprint(group, &originals),
        artifact_lengths: originals.each_ref().map(|b| b.len() as u64),
        decision,
    };
    (intent, originals)
}

pub(super) fn read(s: &State, intent: &Intent, reference: &CollectionReference) -> Result<()> {
    if let Some(error) = s.unavailable {
        return Err(error);
    }
    if !reference.validate()
        || *reference != intent.reference()
        || s.references.get(&intent.operation) != Some(reference)
        || s.evidence.get(&intent.operation) != Some(intent)
        || s.retained_originals
            .get(&intent.operation)
            .is_none_or(|bytes| {
                bytes
                    .iter()
                    .zip([2 * 1024 * 1024, 64 * 1024, 4 * 1024 * 1024])
                    .any(|(b, bound)| b.is_empty() || b.len() > bound)
                    || hex(&fingerprint(intent.group, bytes)) != reference.artifacts_sha256
                    || bytes.each_ref().map(|b| b.len() as u64) != reference.artifact_lengths
            })
    {
        return Err(Refusal::InvalidEvidence);
    }
    Ok(())
}

impl Model {
    pub(super) fn archive_pending(&self, binding: &Binding) -> Result<Option<CollectionReference>> {
        let s = self.state.lock().unwrap();
        if let Some(error) = s.unavailable {
            return Err(error);
        }
        let Some(operation) = s.archive_pending else {
            return Ok(None);
        };
        let intent = s.evidence.get(&operation).ok_or(Refusal::InvalidEvidence)?;
        if intent.binding != *binding {
            return Err(Refusal::StaleAuthority);
        }
        let reference = s
            .references
            .get(&operation)
            .ok_or(Refusal::InvalidEvidence)?;
        read(&s, intent, reference)?;
        Ok(Some(reference.clone()))
    }

    pub(super) fn recover_retained(
        &self,
        expected: &Checkpoint,
        reference: CollectionReference,
    ) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        let operation = uuid::Uuid::parse_str(&reference.operation_id)
            .map_err(|_| Refusal::InvalidEvidence)?
            .into_bytes();
        let intent = s
            .evidence
            .get(&operation)
            .cloned()
            .ok_or(Refusal::InvalidEvidence)?;
        read(&s, &intent, &reference)?;
        let page = s
            .retained_pages
            .get(&operation)
            .cloned()
            .ok_or(Refusal::InvalidEvidence)?;
        if expected.has_work()
            || expected.complete
            || expected.binding != intent.binding
            || expected.source_revision != intent.before_revision
            || expected
                .after_group
                .is_some_and(|after| after >= intent.group)
            || expected.cursor != page.cursor
            || expected
                .page
                .as_ref()
                .is_some_and(|current| *current != page)
            || page.groups.first() != Some(&intent.group)
        {
            return Err(Refusal::Conflict);
        }
        s.checkpoint.page = Some(page);
        s.checkpoint.staged = Some(Staged {
            intent: intent.clone(),
            reference: Some(reference),
        });
        s.checkpoint.revision += 1;
        s.staged = Some(intent);
        Ok(())
    }

    pub(super) fn archive_retain(
        &self,
        intent: &Intent,
        bytes: [Vec<u8>; 3],
        page: Page,
    ) -> Result<CollectionReference> {
        let mut s = self.state.lock().unwrap();
        if let Some(error) = s.unavailable {
            return Err(error);
        }
        if intent.decision != Decision::SettledDeleted {
            return Err(Refusal::InvalidEvidence);
        }
        if s.evidence.contains_key(&intent.operation) {
            read(&s, intent, &intent.reference())?;
            return Ok(intent.reference());
        }
        if s.archive_pending.is_some() || s.checkpoint.binding != intent.binding {
            return Err(Refusal::Conflict);
        }
        if fingerprint(intent.group, &bytes) != intent.evidence_sha256 {
            return Err(Refusal::InvalidEvidence);
        }
        s.retained_pages.insert(intent.operation, page);
        s.references.insert(intent.operation, intent.reference());
        s.evidence.insert(intent.operation, intent.clone());
        s.retained_originals.insert(intent.operation, bytes);
        s.archive_pending = Some(intent.operation);
        s.retentions += 1;
        read(&s, intent, &intent.reference())?;
        fail_after(&mut s, Call::Retain)?;
        Ok(intent.reference())
    }

    pub(super) fn archive_complete(
        &self,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> Result<Receipt> {
        let mut s = self.state.lock().unwrap();
        read(&s, intent, reference)?;
        if let Some(receipt) = s.cold.get(&intent.operation) {
            return Ok(receipt.clone());
        }
        if s.archive_pending != Some(intent.operation) {
            return Err(Refusal::UnknownOutcome);
        }
        let receipt = if let Some(original) = &s.source_committed {
            if !original.matches(intent) {
                return Err(Refusal::UnknownOutcome);
            }
            original.clone()
        } else {
            if s.source_revision != intent.before_revision {
                return Err(Refusal::UnknownOutcome);
            }
            if s.groups.get(&intent.group) != Some(&Decision::SettledDeleted) {
                return Err(Refusal::InvalidEvidence);
            }
            s.groups.remove(&intent.group);
            s.source_revision += 1;
            s.mutations += 1;
            s.retired_operations += 6;
            s.retired_authority += 3;
            let receipt = Receipt {
                version: 1,
                binding_sha256: intent.binding.sha256,
                operation: intent.operation,
                plan_sha256: intent.plan_sha256,
                group: intent.group,
                source_sha256: intent.source_sha256,
                observation_sha256: intent.observation_sha256,
                before_sha256: digest(intent.before_revision),
                after_sha256: digest(s.source_revision),
                before_revision: intent.before_revision,
                after_revision: s.source_revision,
                operations: 6,
                authority: 3,
                cleanup: 0,
                free_pages_before: 32,
                free_pages_after: 33,
                phase: Phase::Acknowledged,
                sha256: Sha256::digest(format!("model final receipt {:?}", intent.operation))
                    .into(),
            };
            // Internal A outcome; B cannot read this as a public phase receipt.
            s.source_committed = Some(receipt.clone());
            fail_after(&mut s, Call::Source)?;
            receipt
        };
        // One archive transaction in the contract double: final receipt,
        // completed group and pending-authority removal become visible together.
        s.cold.insert(intent.operation, receipt.clone());
        s.archive_pending = None;
        s.source_committed = None;
        fail_after(&mut s, Call::Cold)?;
        Ok(receipt)
    }
}
