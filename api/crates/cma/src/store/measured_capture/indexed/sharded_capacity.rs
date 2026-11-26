//! Capacity accounting for a series split across independent custody manifests.

use super::model::{MAX_DAYS, MAX_SERIES_HOUR_ROWS};
use super::CaptureError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const SHARDS: u8 = u64::BITS as u8;
const ROWS_PER_SEGMENT: usize = MAX_SERIES_HOUR_ROWS / SHARDS as usize;
const _: () = assert!(MAX_SERIES_HOUR_ROWS == ROWS_PER_SEGMENT * SHARDS as usize);

pub(super) struct SamplePartition {
    pub preferred: u8,
    pub fallback: u8,
}

impl SamplePartition {
    pub(super) fn for_sample(physical_sample_id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"cma-measured-custody-shard-v2\0");
        hasher.update(physical_sample_id.as_bytes());
        let bytes = hasher.finalize();
        Self {
            preferred: bytes[0] % SHARDS,
            fallback: bytes[1] % SHARDS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Permit {
    Register(u8),
    Spend { segment: u8, used: usize },
}

/// A segment is free to initialize only while its bit has never been set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PermitDirectory {
    registered: u64,
}

impl PermitDirectory {
    fn bit(segment: u8) -> Result<u64, CaptureError> {
        1_u64
            .checked_shl(u32::from(segment))
            .ok_or(CaptureError::Corrupt)
    }

    pub(super) fn register(&mut self, segment: u8) -> Result<(), CaptureError> {
        let bit = Self::bit(segment)?;
        if self.registered & bit != 0 {
            return Err(CaptureError::Corrupt);
        }
        self.registered |= bit;
        Ok(())
    }

    pub(super) fn selected(
        &self,
        segment: u8,
        used: Option<usize>,
    ) -> Result<Option<Permit>, CaptureError> {
        let registered = self.registered & Self::bit(segment)? != 0;
        match (registered, used) {
            (false, None) => Ok(Some(Permit::Register(segment))),
            (true, Some(used)) if (1..ROWS_PER_SEGMENT).contains(&used) => {
                Ok(Some(Permit::Spend { segment, used }))
            }
            (true, Some(ROWS_PER_SEGMENT)) => Ok(None),
            _ => Err(CaptureError::Corrupt),
        }
    }

    /// A full decision requires the complete set of initialized segments.
    pub(super) fn alternative(
        &self,
        offset: u8,
        usage: &BTreeMap<u8, usize>,
    ) -> Result<Option<Permit>, CaptureError> {
        Self::bit(offset)?;
        self.verify_usage(usage)?;
        for distance in 0..SHARDS {
            let segment = (offset + distance) % SHARDS;
            if let Some(permit) = self.selected(segment, usage.get(&segment).copied())? {
                return Ok(Some(permit));
            }
        }
        Ok(None)
    }

    pub(super) fn verify_usage(&self, usage: &BTreeMap<u8, usize>) -> Result<(), CaptureError> {
        if usage.len() != self.registered.count_ones() as usize {
            return Err(CaptureError::Corrupt);
        }
        for (&segment, &used) in usage {
            if self.registered & Self::bit(segment)? == 0 || !(1..=ROWS_PER_SEGMENT).contains(&used)
            {
                return Err(CaptureError::Corrupt);
            }
        }
        Ok(())
    }
}

/// Register each calendar day once, regardless of how many shards have its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DayDirectory {
    days: BTreeSet<u64>,
}

impl DayDirectory {
    pub(super) fn validate(&self) -> Result<(), CaptureError> {
        if self.days.len() > MAX_DAYS {
            return Err(CaptureError::Corrupt);
        }
        Ok(())
    }

    pub(super) fn register(&mut self, day: u64) -> Result<bool, CaptureError> {
        self.validate()?;
        if self.days.contains(&day) {
            return Ok(false);
        }
        if self.days.len() == MAX_DAYS {
            return Err(CaptureError::Unavailable);
        }
        Ok(self.days.insert(day))
    }

    pub(super) fn verify_days(&self, observed: &BTreeSet<u64>) -> Result<(), CaptureError> {
        self.validate()?;
        if &self.days != observed {
            return Err(CaptureError::Corrupt);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_identity_determines_both_partitions() {
        let first = SamplePartition::for_sample("first");
        let repeated = SamplePartition::for_sample("first");
        assert_eq!(first.preferred, repeated.preferred);
        assert_eq!(first.fallback, repeated.fallback);
        let discovered: BTreeSet<_> = (0..1024)
            .map(|i| SamplePartition::for_sample(&format!("sample-{i}")).preferred)
            .collect();
        assert_eq!(discovered, (0..SHARDS).collect());
    }

    #[test]
    fn a_missing_registered_segment_cannot_be_spent_or_reinitialized() {
        let mut directory = PermitDirectory::default();
        assert_eq!(
            directory.selected(7, None).unwrap(),
            Some(Permit::Register(7))
        );
        directory.register(7).unwrap();
        assert!(matches!(
            directory.selected(7, None),
            Err(CaptureError::Corrupt)
        ));
        assert!(matches!(directory.register(7), Err(CaptureError::Corrupt)));
        assert!(matches!(
            directory.alternative(4, &BTreeMap::new()),
            Err(CaptureError::Corrupt)
        ));
        assert_eq!(
            directory.selected(7, Some(3)).unwrap(),
            Some(Permit::Spend {
                segment: 7,
                used: 3
            })
        );
    }

    #[test]
    fn fallback_can_use_any_unspent_permit_before_declaring_the_series_full() {
        let mut directory = PermitDirectory::default();
        let mut usage = BTreeMap::new();
        for segment in 0..SHARDS {
            directory.register(segment).unwrap();
            usage.insert(segment, ROWS_PER_SEGMENT);
        }
        assert_eq!(usage.values().sum::<usize>(), MAX_SERIES_HOUR_ROWS);
        assert_eq!(directory.alternative(32, &usage).unwrap(), None);
        usage.insert(31, ROWS_PER_SEGMENT - 1);
        assert_eq!(
            directory.alternative(32, &usage).unwrap(),
            Some(Permit::Spend {
                segment: 31,
                used: ROWS_PER_SEGMENT - 1
            })
        );
        usage.remove(&19);
        assert!(matches!(
            directory.alternative(32, &usage),
            Err(CaptureError::Corrupt)
        ));
        usage.insert(64, ROWS_PER_SEGMENT);
        assert!(matches!(
            directory.alternative(32, &usage),
            Err(CaptureError::Corrupt)
        ));
    }

    #[test]
    fn each_day_consumes_the_shared_limit_once() {
        let mut directory = DayDirectory::default();
        for day in 0..MAX_DAYS as u64 {
            assert!(directory.register(day).unwrap());
            assert!(!directory.register(day).unwrap());
        }
        assert!(matches!(
            directory.register(MAX_DAYS as u64),
            Err(CaptureError::Unavailable)
        ));
        assert!(!directory.register(0).unwrap());
    }
}
