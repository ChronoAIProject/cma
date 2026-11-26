use super::*;

fn quantity(value: u64) -> ExactQuantity {
    ExactQuantity {
        numerator: value.to_string(),
        denominator: "1".into(),
    }
}

fn fixture() -> Input {
    let interval = Interval {
        issuer: "issuer".into(),
        owner: "owner".into(),
        sandbox: "sandbox".into(),
        resource: "retained-home".into(),
        generation: "home-generation".into(),
        metric: Metric::Disk,
        from_ms: 0,
        to_ms: 60_000,
        window: "window".into(),
    };
    let prefix = Prefix {
        contract: "usage-v1".into(),
        scope: "producer".into(),
        destination: "receiver".into(),
        stream: "stream".into(),
        cutoff_ms: 60_000,
        sequence: 2,
        digest: "payload".into(),
    };
    Input {
        key: Key {
            interval: interval.clone(),
            cohort_digest: "cohort".into(),
            revision: 1,
        },
        predecessor_digest: None,
        cutoff_ms: 60_000,
        allocation: quantity(100),
        measured: Some(quantity(10)),
        coverage: MetricCoverage {
            expected_ms: 60_000,
            measured_ms: 60_000,
            ..Default::default()
        },
        measured_revision: "measured".into(),
        allocation_watermark: "allocation".into(),
        delivery: Some(Delivery {
            receipt: Some(prefix.clone()),
            custody: Some(prefix.clone()),
            validated: Some(prefix.clone()),
            outbox: prefix,
            source_matches: true,
        }),
        funding: Some(Funding {
            interval,
            contract: "funding-v1".into(),
            rate_version: "rate-v1".into(),
            digest: "balances".into(),
            upper_bound: quantity(100),
            balances_match: true,
        }),
    }
}

impl SnapshotWriter for Option<Snapshot> {
    fn append(&mut self, snapshot: Snapshot) -> Result<&Snapshot, Conflict> {
        if self.as_ref().is_some_and(|existing| existing != &snapshot) {
            return Err(Conflict);
        }
        Ok(self.get_or_insert(snapshot))
    }
}

#[test]
fn low_utilization_has_stable_digest_and_cannot_be_overwritten() {
    let mut input = fixture();
    let mut store = None;
    let original = store.append(evaluate(&input).unwrap()).unwrap();
    assert_eq!(original.decision, Decision::Matched);
    assert_eq!(original, &evaluate(&input).unwrap());
    input.measured = Some(quantity(101));
    assert_eq!(evaluate(&input).unwrap().decision, Decision::Fail);
    assert_eq!(store.append(evaluate(&input).unwrap()), Err(Conflict));
}

#[test]
fn local_outbox_and_custody_do_not_establish_validation_or_funding() {
    let mut input = fixture();
    input.delivery.as_mut().unwrap().validated = None;
    let expected = Decision::Blocked(MissingEvidence::Delivery);
    assert_eq!(evaluate(&input).unwrap().decision, expected);
    input.funding = None;
    let expected = Decision::Blocked(MissingEvidence::Funding);
    assert_eq!(evaluate(&input).unwrap().decision, expected);
}
