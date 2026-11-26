//! Residual cohorts share exact integer sums. Fractional or signed values
//! switch the affected statistic to the original per-cutoff ordered fold.
use super::*;
use layer2_core::sandbox_metrics::series::Bucket;
use std::collections::HashMap;

pub(super) const MAX_TOP: usize = 20;
const MAX_EXACT: u64 = 1 << 53;

#[derive(Clone)]
enum Sums {
    Exact { total: u64, ends: [u64; MAX_TOP] },
    Ordered([f64; MAX_TOP]),
}

#[derive(Clone)]
struct PrefixFloat {
    active: usize,
    sums: Sums,
}
impl Default for PrefixFloat {
    fn default() -> Self {
        Self {
            active: 0,
            sums: Sums::Exact {
                total: 0,
                ends: [0; MAX_TOP],
            },
        }
    }
}
impl PrefixFloat {
    fn add(&mut self, count: usize, value: f64) {
        let previous = self.active;
        self.active = self.active.max(count);
        if let Sums::Exact { total, ends } = &mut self.sums {
            // All included values and every intermediate nonnegative sum
            // are exactly representable, independent of addition order.
            let integer = value as u64;
            if value.is_sign_positive()
                && value == integer as f64
                && integer <= MAX_EXACT
                && *total <= MAX_EXACT - integer
            {
                *total += integer;
                ends[count - 1] += integer;
                return;
            }
            let mut ordered = [0.; MAX_TOP];
            let mut running = 0;
            for (index, value) in ends.iter().enumerate().rev() {
                running += value;
                ordered[index] = running as f64;
            }
            self.sums = Sums::Ordered(ordered);
        }
        if let Sums::Ordered(sums) = &mut self.sums {
            for (index, sum) in sums[..count].iter_mut().enumerate() {
                if index < previous {
                    *sum += value;
                } else {
                    *sum = value;
                }
            }
        }
    }

    fn value(&self, cutoff: usize) -> Option<f64> {
        (cutoff < self.active).then(|| match &self.sums {
            Sums::Exact { ends, .. } => ends[cutoff..].iter().sum::<u64>() as f64,
            Sums::Ordered(values) => values[cutoff],
        })
    }
}

#[derive(Default)]
struct ResidualBucket {
    active: usize,
    zero_prefix: usize,
    from_to: [(u64, u64); MAX_TOP],
    integral: PrefixFloat,
    display: PrefixFloat,
    capacity_usage: PrefixFloat,
    capacity: PrefixFloat,
    covered: PrefixFloat,
    expected: PrefixFloat,
    unknown: PrefixFloat,
    mean: PrefixFloat,
    min: PrefixFloat,
    max: PrefixFloat,
    excluded: [u64; MAX_TOP],
    excluded_total: u64,
}
impl ResidualBucket {
    fn add(&mut self, count: usize, value: &Bucket) -> Result<()> {
        self.excluded_total = self
            .excluded_total
            .checked_add(value.excluded_count)
            .ok_or(MetricsError::Unavailable)?;
        if count > self.active {
            self.from_to[self.active..count].fill((value.from, value.to));
            self.active = count;
        }
        let measurement_bits = value.integral.to_bits()
            | value.display_integral.to_bits()
            | value.capacity_usage_integral.to_bits()
            | value.capacity_integral.to_bits()
            | value.covered_seconds.to_bits();
        if measurement_bits == 0 {
            // The same missing measurement contributes +0 to all five sums.
            // Track that prefix once. A +0 anywhere in an otherwise negative
            // zero sum makes its result +0, independent of its position.
            self.zero_prefix = self.zero_prefix.max(count);
        } else {
            self.integral.add(count, value.integral);
            self.display.add(count, value.display_integral);
            self.capacity_usage
                .add(count, value.capacity_usage_integral);
            self.capacity.add(count, value.capacity_integral);
            self.covered.add(count, value.covered_seconds);
        }
        self.expected.add(count, value.expected_seconds);
        self.unknown.add(count, value.unknown_mapping_seconds);
        if let Some(mean) = value.mean {
            self.mean.add(count, mean);
        }
        if let Some(min) = value.min {
            self.min.add(count, min);
        }
        if let Some(max) = value.max {
            self.max.add(count, max);
        }
        self.excluded[count - 1] = self.excluded[count - 1]
            .checked_add(value.excluded_count)
            .ok_or(MetricsError::Unavailable)?;
        Ok(())
    }

    fn finish(&self, cutoff: usize) -> Bucket {
        let (from, to) = self.from_to[cutoff];
        let measured = |sum: &PrefixFloat| {
            let result = sum.value(cutoff).unwrap_or_default();
            if result == 0. && cutoff < self.zero_prefix {
                0.
            } else {
                result
            }
        };
        Bucket {
            from,
            to,
            integral: measured(&self.integral),
            display_integral: measured(&self.display),
            capacity_usage_integral: measured(&self.capacity_usage),
            capacity_integral: measured(&self.capacity),
            covered_seconds: measured(&self.covered),
            expected_seconds: self.expected.value(cutoff).unwrap_or_default(),
            unknown_mapping_seconds: self.unknown.value(cutoff).unwrap_or_default(),
            mean: self.mean.value(cutoff),
            min: self.min.value(cutoff),
            max: self.max.value(cutoff),
            excluded_count: self.excluded[cutoff..].iter().sum(),
        }
    }
}

pub(super) struct RankedBuckets {
    ranks: HashMap<String, usize>,
    present: usize,
    buckets: Vec<ResidualBucket>,
}
impl RankedBuckets {
    pub(super) fn new(ranks: Vec<String>) -> Self {
        Self {
            ranks: ranks
                .into_iter()
                .enumerate()
                .map(|(index, id)| (id, index))
                .collect(),
            present: 0,
            buckets: Vec::new(),
        }
    }

    // Each Sandbox contributes to a prefix of cutoff cohorts.
    pub(super) fn add_sandbox(&mut self, id: &str, points: &[Bucket]) -> Result<()> {
        let count = if id == "other" {
            MAX_TOP
        } else {
            self.ranks.get(id).copied().unwrap_or(MAX_TOP)
        };
        self.present = self.present.max(count);
        if count == 0 || points.is_empty() {
            return Ok(());
        }
        if self.buckets.is_empty() {
            self.buckets
                .resize_with(points.len(), ResidualBucket::default);
        }
        if points.len() != self.buckets.len() {
            return Err(MetricsError::Unavailable);
        }
        for (bucket, value) in self.buckets.iter_mut().zip(points) {
            bucket.add(count, value)?;
        }
        Ok(())
    }

    pub(super) fn into_other(self) -> [Option<Vec<Bucket>>; MAX_TOP] {
        std::array::from_fn(|cutoff| {
            (cutoff < self.present).then(|| {
                self.buckets
                    .iter()
                    .filter(|bucket| cutoff < bucket.active)
                    .map(|bucket| bucket.finish(cutoff))
                    .collect()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::usage_reports::charts::merge_buckets;

    #[test]
    fn integer_prefix_promotes_to_exact_ordered_float_sums() {
        let mut prefix = PrefixFloat::default();
        let mut expected = [None; MAX_TOP];
        let inputs = [
            (3, 0.0),
            (20, 17.0),
            (11, (MAX_EXACT - 17) as f64),
            (1, 1.0),
            (15, -0.0),
            (7, f64::from_bits(1)),
            (13, -0.5),
            (20, 1e14),
            (2, 0.125),
        ];
        for (count, value) in inputs {
            prefix.add(count, value);
            for sum in &mut expected[..count] {
                *sum = Some(sum.map_or(value, |prior| prior + value));
            }
            for (index, value) in expected.iter().enumerate() {
                assert_eq!(
                    prefix.value(index).map(f64::to_bits),
                    value.map(f64::to_bits)
                );
            }
        }
    }

    #[test]
    fn missing_measurements_preserve_signed_zero_per_cutoff() {
        let mut residual = ResidualBucket::default();
        let signed = Bucket {
            integral: -0.,
            display_integral: -0.,
            capacity_usage_integral: -0.,
            capacity_integral: -0.,
            covered_seconds: -0.,
            ..Bucket::default()
        };
        residual.add(1, &Bucket::default()).unwrap();
        residual.add(4, &signed).unwrap();
        residual.add(2, &Bucket::default()).unwrap();
        for cutoff in 0..4 {
            let expected = if cutoff < 2 { 0_f64 } else { -0_f64 };
            let bucket = residual.finish(cutoff);
            for value in [
                bucket.integral,
                bucket.display_integral,
                bucket.capacity_usage_integral,
                bucket.capacity_integral,
                bucket.covered_seconds,
            ] {
                assert_eq!(value.to_bits(), expected.to_bits(), "cutoff={cutoff}");
            }
        }
    }

    #[test]
    fn every_cutoff_preserves_all_bucket_fields_and_order() {
        let ranks: Vec<_> = (0..MAX_TOP).map(|index| format!("rank-{index}")).collect();
        let mut projection = RankedBuckets::new(ranks.clone());
        let mut expected: [Option<Vec<Bucket>>; MAX_TOP] = std::array::from_fn(|_| None);
        let cases = [
            0.,
            1.,
            16.,
            -0.,
            1.5,
            -0.125,
            f64::from_bits(1),
            1e14,
            MAX_EXACT as f64,
        ];
        for index in 0..300 {
            let identity = if index % 3 == 0 {
                ranks[index % MAX_TOP].as_str()
            } else if index % 7 == 0 {
                "other"
            } else {
                "unranked"
            };
            let buckets: Vec<_> = (0..12)
                .map(|bucket| {
                    let value = cases[(index + bucket) % cases.len()];
                    Bucket {
                        from: (bucket * 60) as u64,
                        to: (bucket * 60 + 60) as u64,
                        integral: value,
                        display_integral: cases[(index + bucket + 1) % cases.len()],
                        capacity_usage_integral: cases[(index + bucket + 2) % cases.len()],
                        capacity_integral: 60.,
                        covered_seconds: if index % 3 == 0 { 0. } else { 60. },
                        expected_seconds: 60.,
                        unknown_mapping_seconds: if index % 5 == 0 { 0. } else { 60. },
                        mean: (index % 7 != 0).then_some(value),
                        min: (index % 5 != 0).then_some(value),
                        max: (index % 3 != 0).then_some(value),
                        excluded_count: u64::from(index % 3 == 0),
                    }
                })
                .collect();
            projection.add_sandbox(identity, &buckets).unwrap();
            for (cutoff, target) in expected.iter_mut().enumerate() {
                if identity == "other" || !ranks[..=cutoff].iter().any(|id| id == identity) {
                    merge_buckets(target.get_or_insert_with(Vec::new), &buckets);
                }
            }
        }
        for (cached, legacy) in projection.into_other().into_iter().zip(expected) {
            assert_eq!(
                mongodb::bson::serialize_to_vec(&doc! {"line": encode(&cached).unwrap()}).unwrap(),
                mongodb::bson::serialize_to_vec(&doc! {"line": encode(&legacy).unwrap()}).unwrap()
            );
        }
    }
}
