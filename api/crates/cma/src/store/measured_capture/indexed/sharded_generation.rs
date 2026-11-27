//! Fixed root membership and versioned keys for partitioned custody.

use super::model::{canonical_number, valid_digest, Certification, Phase, MAX_DAYS};
use super::sharded_capacity::SHARDS;
use super::CaptureError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const VERSION: i64 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ShardedGeneration {
    pub generation: u64,
    pub revision: u64,
    pub phase: Phase,
    pub writer_floor_sha256: String,
    pub predecessor_sha256: Option<String>,
    pub cursor: Option<String>,
    pub backfilled_rows: u64,
    pub certification: Option<Certification>,
}

impl ShardedGeneration {
    pub(super) fn new(
        generation: u64,
        writer_floor_sha256: String,
        predecessor_sha256: Option<String>,
    ) -> Result<Self, CaptureError> {
        let value = Self {
            generation,
            revision: 1,
            phase: Phase::Building,
            writer_floor_sha256,
            predecessor_sha256,
            cursor: None,
            backfilled_rows: 0,
            certification: None,
        };
        value.validate()?;
        Ok(value)
    }

    pub(super) fn validate(&self) -> Result<(), CaptureError> {
        if self.generation == 0
            || self.revision == 0
            || !valid_digest(&self.writer_floor_sha256)
            || self
                .predecessor_sha256
                .as_ref()
                .is_some_and(|digest| !valid_digest(digest))
            || (self.generation == 1) != self.predecessor_sha256.is_none()
            || self.cursor.is_none() != (self.backfilled_rows == 0)
            || (self.phase == Phase::Ready) != self.certification.is_some()
            || self.certification.as_ref().is_some_and(|proof| {
                proof.original_rows != self.backfilled_rows || !valid_digest(&proof.ordered_sha256)
            })
        {
            return Err(CaptureError::Corrupt);
        }
        Ok(())
    }

    pub(super) fn verify_shards(
        &self,
        roots: impl IntoIterator<Item = ShardRoot>,
        sealed: bool,
    ) -> Result<BTreeMap<u8, ShardRoot>, CaptureError> {
        let mut expected = BTreeMap::new();
        let mut rows = 0_u64;
        for root in roots {
            root.validate(self.generation)?;
            if root.sealed != sealed {
                return Err(CaptureError::Corrupt);
            }
            rows = rows.checked_add(root.rows).ok_or(CaptureError::Corrupt)?;
            if expected.insert(root.shard, root).is_some() {
                return Err(CaptureError::Corrupt);
            }
        }
        if expected.len() != usize::from(SHARDS)
            || self
                .certification
                .as_ref()
                .is_some_and(|proof| proof.original_rows > rows)
        {
            return Err(CaptureError::Corrupt);
        }
        Ok(expected)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ShardRoot {
    pub generation: u64,
    pub shard: u8,
    pub revision: u64,
    pub sealed: bool,
    pub rows: u64,
    pub days: BTreeMap<String, String>,
}

impl ShardRoot {
    pub(super) fn new(generation: u64, shard: u8) -> Result<Self, CaptureError> {
        let root = Self {
            generation,
            shard,
            revision: 1,
            sealed: false,
            rows: 0,
            days: BTreeMap::new(),
        };
        root.validate(generation)?;
        Ok(root)
    }

    pub(super) fn validate(&self, generation: u64) -> Result<(), CaptureError> {
        if generation == 0
            || self.generation != generation
            || self.shard >= SHARDS
            || self.revision == 0
            || self.days.len() > MAX_DAYS
            || self.days.values().any(|digest| !valid_digest(digest))
            || self.days.keys().any(|day| canonical_number(day).is_none())
        {
            return Err(CaptureError::Corrupt);
        }
        Ok(())
    }
}

pub(super) fn shard_id(generation: u64, shard: u8) -> String {
    format!("shard:{generation}:{shard}")
}

pub(super) fn day_id(generation: u64, shard: u8, day: u64) -> String {
    format!("shard-day:{generation}:{shard}:{day}")
}

pub(super) fn hour_id(generation: u64, shard: u8, hour: u64) -> String {
    format!("shard-hour:{generation}:{shard}:{hour}")
}

pub(super) fn series_id(generation: u64, shard: u8, hour: u64, series: &str) -> String {
    format!("shard-series:{generation}:{shard}:{hour}:{series}")
}

pub(super) fn calendar_id(generation: u64) -> String {
    format!("shard-calendar:{generation}")
}

pub(super) fn predecessor_id(generation: u64) -> String {
    format!("shard-predecessor:{generation}")
}

pub(super) fn permits_id(generation: u64, hour: u64, series: &str) -> String {
    format!("permits:{generation}:{hour}:{series}")
}

pub(super) fn segment_id(generation: u64, hour: u64, series: &str, permit: u8) -> String {
    format!("permit-segment:{generation}:{hour}:{series}:{permit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(generation: u64) -> Vec<ShardRoot> {
        (0..SHARDS)
            .map(|shard| ShardRoot::new(generation, shard).unwrap())
            .collect()
    }

    #[test]
    fn all_shards_including_empty_shards_are_part_of_the_generation() {
        let directory = ShardedGeneration::new(2, "a".repeat(64), Some("b".repeat(64))).unwrap();
        let complete = roots(2);
        assert_eq!(
            directory
                .verify_shards(complete.clone(), false)
                .unwrap()
                .len(),
            usize::from(SHARDS)
        );
        for invalid in [
            complete[..63].to_vec(),
            roots(3),
            [complete[..63].to_vec(), vec![complete[0].clone()]].concat(),
        ] {
            assert!(matches!(
                directory.verify_shards(invalid, false),
                Err(CaptureError::Corrupt)
            ));
        }
    }

    #[test]
    fn a_sealing_receipt_requires_every_shard_and_the_certified_row_floor() {
        let mut directory =
            ShardedGeneration::new(2, "a".repeat(64), Some("b".repeat(64))).unwrap();
        directory.phase = Phase::Ready;
        directory.cursor = Some("sample:fixture".into());
        directory.backfilled_rows = 1;
        directory.certification = Some(Certification {
            original_rows: 1,
            ordered_sha256: "b".repeat(64),
        });
        directory.validate().unwrap();
        let mut complete = roots(2);
        for shard in &mut complete {
            shard.sealed = true;
        }
        assert!(matches!(
            directory.verify_shards(complete.clone(), true),
            Err(CaptureError::Corrupt)
        ));
        complete[17].rows = 1;
        assert!(directory.verify_shards(complete.clone(), true).is_ok());
        complete[17].sealed = false;
        assert!(matches!(
            directory.verify_shards(complete, true),
            Err(CaptureError::Corrupt)
        ));
    }
}
