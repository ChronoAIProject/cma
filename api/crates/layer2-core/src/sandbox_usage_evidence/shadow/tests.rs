use super::*;

fn quantity(value: u64) -> ExactQuantity {
    num_rational::Ratio::from_integer(value.into()).into()
}

fn fixture() -> Input {
    let id = |value| Identifier::new(value).unwrap();
    let interval = Interval {
        issuer: id("issuer"),
        owner: id("owner"),
        sandbox: id("sandbox"),
        resource: id("retained-home"),
        generation: id("home-generation"),
        metric: Metric::Disk,
        from_ms: 0,
        to_ms: 60_000,
        window: id("window"),
    };
    let prefix = Prefix {
        contract: id("usage-v1"),
        scope: id("producer"),
        destination: id("receiver"),
        stream: id("stream"),
        cutoff_ms: 60_000,
        sequence: 2,
        digest: id("payload"),
    };
    Input {
        key: Key {
            interval: interval.clone(),
            cohort_digest: id("cohort"),
            revision: 1,
        },
        predecessor_digest: None,
        cutoff_ms: 60_000,
        allocation: quantity(100),
        measured: Some(quantity(10)),
        status: MeasurementStatus::Measured,
        measured_revision: Some(id("measured")),
        allocation_watermark: Some(id("allocation")),
        delivery: Some(Delivery {
            receipt: Some(prefix.clone()),
            custody: Some(prefix.clone()),
            validated: Some(prefix.clone()),
            outbox: prefix,
            source_matches: true,
        }),
        funding: Some(Funding {
            interval,
            contract: id("funding-v1"),
            rate_version: id("rate-v1"),
            digest: id("balances"),
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
fn observed_usage_replays_and_invalid_inactive_coverage_conflicts() {
    let mut input = fixture();
    let mut store = None;
    let original = store.append(evaluate(&input).unwrap()).unwrap();
    assert_eq!(original.decision, Decision::Matched);
    assert_eq!(original, &evaluate(&input).unwrap());
    input.measured = Some(quantity(101));
    assert_eq!(evaluate(&input).unwrap().decision, Decision::Fail);
    assert_eq!(store.append(evaluate(&input).unwrap()), Err(Conflict));
    input.measured = None;
    input.status = MeasurementStatus::NotRunning;
    assert_eq!(evaluate(&input), Err(Conflict));
    input.key.interval.metric = Metric::Cpu;
    input.allocation = quantity(0);
    input.funding.as_mut().unwrap().interval = input.key.interval.clone();
    assert_eq!(evaluate(&input).unwrap().decision, Decision::Matched);
}

#[test]
fn proof_requires_nonblank_identity_and_remote_validation() {
    assert_eq!(Identifier::new(""), Err(Conflict));
    assert_eq!(Identifier::new(" \t"), Err(Conflict));
    let mut input = fixture();
    input.delivery.as_mut().unwrap().validated = None;
    let expected = Decision::Blocked(MissingEvidence::Delivery);
    assert_eq!(evaluate(&input).unwrap().decision, expected);
    input.funding = None;
    let expected = Decision::Blocked(MissingEvidence::Funding);
    assert_eq!(evaluate(&input).unwrap().decision, expected);
}
